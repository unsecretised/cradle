//! Named tool results and the text filters that work on them. See
//! `.agents/roadmap/FEATURE_TOOL_VARIABLES.md`.
//!
//! A call with `save_as` stores its full result here, and the model gets a short summary
//! instead. The `var_*` tools filter stored text without touching the file system. Stored
//! text is what the model would have seen anyway (hygiene already applied), so nothing
//! here needs the path guard.

use std::collections::{BTreeMap, HashSet};

use regex::Regex;
use serde_json::Value;

use crate::ToolError;

/// The largest result one variable may hold.
pub const MAX_VAR_BYTES: usize = 2 * 1024 * 1024;
/// The most all variables together may hold.
pub const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
/// The longest variable name.
pub const MAX_NAME_LEN: usize = 32;
/// Lines of a saved result shown in the summary.
pub const SAVE_PREVIEW_LINES: usize = 5;
/// The most lines a `var_*` tool returns to the model.
pub const MAX_SHOW_LINES: usize = 2000;
/// Preview lines are cut to this many characters.
const PREVIEW_WIDTH: usize = 200;

/// One saved result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub text: String,
    /// The call that produced it (its summary), e.g. `search /fn/ in .`.
    pub source: String,
}

/// The variables saved in this conversation.
#[derive(Debug, Default)]
pub struct VarStore {
    vars: BTreeMap<String, Variable>,
}

impl VarStore {
    /// A variable by name (a leading `$` is allowed).
    pub fn get(&self, name: &str) -> Result<&Variable, ToolError> {
        let name = name.strip_prefix('$').unwrap_or(name);
        self.vars.get(name).ok_or_else(|| {
            let saved = if self.vars.is_empty() {
                "none are saved".to_owned()
            } else {
                format!("saved: {}", self.names().join(", "))
            };
            ToolError::new(format!("no variable `${name}` ({saved})"))
        })
    }

    /// Save `text` as `name` (replacing any variable of that name), and return the
    /// summary the model gets instead of the text.
    pub fn save(&mut self, name: &str, text: String, source: String) -> Result<String, ToolError> {
        let name = check_name(name).map_err(ToolError)?;
        if text.len() > MAX_VAR_BYTES {
            return Err(ToolError::new(format!(
                "not saved: the result is {} and a variable holds at most {}. Narrow the call \
                 (a path, glob or line range) and try again.",
                size(text.len()),
                size(MAX_VAR_BYTES)
            )));
        }
        let others: usize = self
            .vars
            .iter()
            .filter(|(n, _)| **n != name)
            .map(|(_, v)| v.text.len())
            .sum();
        if others + text.len() > MAX_TOTAL_BYTES {
            return Err(ToolError::new(format!(
                "not saved: variables would hold {} in total (the limit is {}). Drop some \
                 with var_list first.",
                size(others + text.len()),
                size(MAX_TOTAL_BYTES)
            )));
        }
        let summary = summary(&name, &text, &source);
        self.vars.insert(name, Variable { text, source });
        Ok(summary)
    }

    /// Remove variables by name. Returns the names that did not exist.
    pub fn drop_vars(&mut self, names: &[String]) -> Vec<String> {
        names
            .iter()
            .map(|n| n.strip_prefix('$').unwrap_or(n))
            .filter(|n| self.vars.remove(*n).is_none())
            .map(|n| format!("${n}"))
            .collect()
    }

    /// One line per variable, for `var_list`.
    pub fn list(&self) -> String {
        if self.vars.is_empty() {
            return "No variables saved.".into();
        }
        let mut out: Vec<String> = self
            .vars
            .iter()
            .map(|(name, v)| {
                let lines = v.text.lines().count();
                format!(
                    "${name}: {lines} {}, {} (from {})",
                    if lines == 1 { "line" } else { "lines" },
                    size(v.text.len()),
                    v.source
                )
            })
            .collect();
        let total: usize = self.vars.values().map(|v| v.text.len()).sum();
        out.push(format!(
            "total: {} of {}",
            size(total),
            size(MAX_TOTAL_BYTES)
        ));
        out.join("\n")
    }

    pub fn clear(&mut self) {
        self.vars.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    fn names(&self) -> Vec<String> {
        self.vars.keys().map(|n| format!("${n}")).collect()
    }
}

/// Check a variable name: `[A-Za-z_][A-Za-z0-9_]*`, at most [`MAX_NAME_LEN`] characters,
/// with an optional leading `$`. Returns it without the `$`.
pub fn check_name(name: &str) -> Result<String, String> {
    let bare = name.strip_prefix('$').unwrap_or(name);
    let mut chars = bare.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && bare.len() <= MAX_NAME_LEN;
    if valid {
        Ok(bare.to_owned())
    } else {
        Err(format!(
            "`{name}` is not a valid variable name (letters, digits and _, not starting with a \
             digit, at most {MAX_NAME_LEN} characters)"
        ))
    }
}

/// Split the optional `save_as` off a tool call's input, so the input can be parsed as
/// usual. Errors are returned to the model.
pub fn take_save_as(input: &Value) -> Result<(Value, Option<String>), String> {
    let mut input = input.clone();
    let Some(raw) = input.as_object_mut().and_then(|o| o.remove("save_as")) else {
        return Ok((input, None));
    };
    let name = raw
        .as_str()
        .ok_or("invalid input: `save_as` must be a string")?;
    let name = check_name(name).map_err(|e| format!("invalid input: {e}"))?;
    Ok((input, Some(name)))
}

/// Compile a pattern for the `var_*` tools.
pub fn regex(pattern: &str) -> Result<Regex, ToolError> {
    Regex::new(pattern).map_err(|e| ToolError::new(format!("invalid regex: {e}")))
}

/// Keep the lines matching `re` (or not matching, with `invert`), with `context` lines
/// around each match. Groups that don't touch are separated by `--`, as in grep. With
/// `only_matching`, return each match on its own line instead.
pub fn grep(text: &str, re: &Regex, context: usize, invert: bool, only_matching: bool) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if only_matching {
        return lines
            .iter()
            .flat_map(|l| re.find_iter(l).map(|m| m.as_str()))
            .collect::<Vec<_>>()
            .join("\n");
    }
    let hits = (0..lines.len()).filter(|&i| re.is_match(lines[i]) != invert);
    let mut out: Vec<&str> = Vec::new();
    // The first line not printed yet.
    let mut next = 0;
    for i in hits {
        let start = i.saturating_sub(context).max(next);
        let end = (i + context + 1).min(lines.len());
        if start >= end {
            continue;
        }
        if context > 0 && !out.is_empty() && start > next {
            out.push("--");
        }
        out.extend(&lines[start..end]);
        next = end;
    }
    out.join("\n")
}

/// Replace every match of `re` in each line (`$1`, `${name}` refer to groups).
pub fn replace(text: &str, re: &Regex, replacement: &str) -> String {
    text.lines()
        .map(|l| re.replace_all(l, replacement))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which lines `var_lines` keeps. Applied in field order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineOptions {
    /// First line to keep (1-based).
    pub from: Option<usize>,
    /// Last line to keep (1-based, inclusive).
    pub to: Option<usize>,
    /// Drop repeated lines, keeping the first.
    pub unique: bool,
    pub sort: bool,
    pub head: Option<usize>,
    pub tail: Option<usize>,
}

pub fn lines(text: &str, o: &LineOptions) -> String {
    let all: Vec<&str> = text.lines().collect();
    let from = o.from.unwrap_or(1).max(1) - 1;
    let to = o.to.unwrap_or(all.len()).min(all.len());
    let mut picked: Vec<&str> = if from < to {
        all[from..to].to_vec()
    } else {
        Vec::new()
    };
    if o.unique {
        let mut seen = HashSet::new();
        picked.retain(|l| seen.insert(*l));
    }
    if o.sort {
        picked.sort_unstable();
    }
    if let Some(n) = o.head {
        picked.truncate(n);
    }
    if let Some(n) = o.tail {
        let skip = picked.len().saturating_sub(n);
        picked.drain(..skip);
    }
    picked.join("\n")
}

/// A `var_*` result as returned to the model: at most [`MAX_SHOW_LINES`] lines.
pub fn cap_output(text: &str) -> String {
    if text.is_empty() {
        return "(no lines)".into();
    }
    let total = text.lines().count();
    if total <= MAX_SHOW_LINES {
        return text.to_owned();
    }
    let mut out = text
        .lines()
        .take(MAX_SHOW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    out.push_str(&format!(
        "\n… {} more lines (save the result with save_as and page through it with var_lines \
         from/to)",
        total - MAX_SHOW_LINES
    ));
    out
}

fn summary(name: &str, text: &str, source: &str) -> String {
    let total = text.lines().count();
    let mut out = format!(
        "saved ${name}: {total} {}, {} (from {source})",
        if total == 1 { "line" } else { "lines" },
        size(text.len())
    );
    if total > 0 {
        out.push(':');
        for line in text.lines().take(SAVE_PREVIEW_LINES) {
            out.push('\n');
            if line.chars().count() > PREVIEW_WIDTH {
                out.extend(line.chars().take(PREVIEW_WIDTH));
                out.push('…');
            } else {
                out.push_str(line);
            }
        }
        if total > SAVE_PREVIEW_LINES {
            out.push_str(&format!("\n… {} more lines", total - SAVE_PREVIEW_LINES));
        }
    }
    out
}

fn size(bytes: usize) -> String {
    const KIB: usize = 1024;
    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < KIB * KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{:.1} MiB", bytes as f64 / (KIB * KIB) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HITS: &str = "src/a.rs:1: fn a() {}\nsrc/b.rs:4: fn b() {}\nsrc/a.rs:9: fn c() {}";

    #[test]
    fn save_returns_a_summary_with_a_preview() {
        let mut store = VarStore::default();
        let text: String = (1..=8).map(|i| format!("line {i}\n")).collect();
        let summary = store
            .save("hits", text.clone(), "search /x/ in .".into())
            .unwrap();
        assert!(
            summary.starts_with("saved $hits: 8 lines, 56 B (from search /x/ in .):\nline 1\n"),
            "{summary}"
        );
        assert!(summary.contains("line 5\n… 3 more lines"), "{summary}");
        assert!(!summary.contains("line 6"), "{summary}");
        assert_eq!(store.get("hits").unwrap().text, text);
        assert_eq!(
            store.get("$hits").unwrap().text,
            text,
            "a leading $ is allowed"
        );

        let empty = store.save("none", String::new(), "s".into()).unwrap();
        assert_eq!(empty, "saved $none: 0 lines, 0 B (from s)");
    }

    #[test]
    fn long_preview_lines_are_cut() {
        let mut store = VarStore::default();
        let summary = store.save("v", "x".repeat(500), "s".into()).unwrap();
        let line = summary.lines().nth(1).unwrap();
        assert_eq!(line.chars().count(), PREVIEW_WIDTH + 1);
        assert!(line.ends_with('…'));
    }

    #[test]
    fn unknown_variables_list_the_saved_ones() {
        let mut store = VarStore::default();
        let err = store.get("x").unwrap_err().0;
        assert!(err.contains("no variable `$x` (none are saved)"), "{err}");
        store.save("a", "1".into(), "s".into()).unwrap();
        store.save("b", "2".into(), "s".into()).unwrap();
        let err = store.get("$x").unwrap_err().0;
        assert!(err.contains("(saved: $a, $b)"), "{err}");
    }

    #[test]
    fn enforces_size_limits() {
        let mut store = VarStore::default();
        let err = store
            .save("big", "x".repeat(MAX_VAR_BYTES + 1), "s".into())
            .unwrap_err()
            .0;
        assert!(err.starts_with("not saved"), "{err}");
        assert!(store.is_empty());

        for i in 0..8 {
            store
                .save(&format!("v{i}"), "x".repeat(MAX_VAR_BYTES), "s".into())
                .unwrap();
        }
        let err = store
            .save("one_more", "x".into(), "s".into())
            .unwrap_err()
            .0;
        assert!(err.contains("in total"), "{err}");
        // Replacing a variable only counts its new size.
        store
            .save("v0", "x".repeat(MAX_VAR_BYTES), "s".into())
            .unwrap();
        assert_eq!(store.drop_vars(&["v0".into(), "$nope".into()]), ["$nope"]);
        store.save("one_more", "x".into(), "s".into()).unwrap();
    }

    #[test]
    fn lists_and_clears_variables() {
        let mut store = VarStore::default();
        assert_eq!(store.list(), "No variables saved.");
        store
            .save("hits", HITS.into(), "search /fn/ in .".into())
            .unwrap();
        let list = store.list();
        assert!(
            list.starts_with(
                "$hits: 3 lines, 65 B (from search /fn/ in .)\ntotal: 65 B of 16.0 MiB"
            ),
            "{list}"
        );
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn checks_names() {
        assert_eq!(check_name("hits").unwrap(), "hits");
        assert_eq!(check_name("$_a1").unwrap(), "_a1");
        for bad in [
            "",
            "$",
            "1a",
            "a-b",
            "a b",
            "ä",
            &"a".repeat(MAX_NAME_LEN + 1),
        ] {
            assert!(check_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn takes_save_as_off_the_input() {
        let (input, name) = take_save_as(&json!({"pattern": "fn", "save_as": "$hits"})).unwrap();
        assert_eq!(
            (input, name.as_deref()),
            (json!({"pattern": "fn"}), Some("hits"))
        );
        let (input, name) = take_save_as(&json!({"pattern": "fn"})).unwrap();
        assert_eq!((input, name), (json!({"pattern": "fn"}), None));
        assert!(take_save_as(&json!({"save_as": 3})).is_err());
        assert!(take_save_as(&json!({"save_as": "no good"})).is_err());
    }

    #[test]
    fn greps_lines() {
        let re = regex("a\\.rs").unwrap();
        assert_eq!(
            grep(HITS, &re, 0, false, false),
            "src/a.rs:1: fn a() {}\nsrc/a.rs:9: fn c() {}"
        );
        assert_eq!(grep(HITS, &re, 0, true, false), "src/b.rs:4: fn b() {}");
        let re = regex("fn \\w").unwrap();
        assert_eq!(grep(HITS, &re, 0, false, true), "fn a\nfn b\nfn c");
        assert_eq!(grep(HITS, &regex("zzz").unwrap(), 2, false, false), "");
        assert!(regex("(").is_err());
    }

    #[test]
    fn greps_with_context_like_grep() {
        let text = "1\n2\nhit\n4\n5\n6\n7\nhit\n9\nhit";
        let re = regex("hit").unwrap();
        assert_eq!(
            grep(text, &re, 1, false, false),
            "2\nhit\n4\n--\n7\nhit\n9\nhit"
        );
        // Overlapping windows are merged without a separator.
        assert_eq!(grep("hit\nhit\nx", &re, 1, false, false), "hit\nhit\nx");
    }

    #[test]
    fn replaces_in_each_line() {
        let re = regex("^([^:]+):.*").unwrap();
        assert_eq!(replace(HITS, &re, "$1"), "src/a.rs\nsrc/b.rs\nsrc/a.rs");
    }

    #[test]
    fn picks_lines_in_order() {
        let text = "c\na\nb\na\nd";
        let all = |o: LineOptions| lines(text, &o);
        assert_eq!(all(LineOptions::default()), text);
        assert_eq!(
            all(LineOptions {
                from: Some(2),
                to: Some(4),
                ..Default::default()
            }),
            "a\nb\na"
        );
        assert_eq!(
            all(LineOptions {
                unique: true,
                sort: true,
                ..Default::default()
            }),
            "a\nb\nc\nd"
        );
        assert_eq!(
            all(LineOptions {
                sort: true,
                head: Some(3),
                tail: Some(2),
                ..Default::default()
            }),
            "a\nb"
        );
        assert_eq!(
            all(LineOptions {
                from: Some(9),
                ..Default::default()
            }),
            ""
        );
    }

    #[test]
    fn caps_output() {
        assert_eq!(cap_output(""), "(no lines)");
        assert_eq!(cap_output("a\nb"), "a\nb");
        let long: String = (0..MAX_SHOW_LINES + 3).map(|i| format!("{i}\n")).collect();
        let capped = cap_output(&long);
        assert_eq!(capped.lines().count(), MAX_SHOW_LINES + 1);
        assert!(capped.ends_with("… 3 more lines (save the result with save_as and page through it with var_lines from/to)"));
    }
}

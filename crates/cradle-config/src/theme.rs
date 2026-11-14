//! The TUI theme in `~/.config/cradle/theme.toml`. Global only: projects cannot set it.
//!
//! This module reads the file's structure into strings. Colours, roles, element names and
//! modifiers are checked by the `cradle` bin, which knows the terminal types. Loading never
//! fails: each problem becomes a warning and the entry is skipped.
//!
//! ```toml
//! preset = "light"
//!
//! [palette]
//! blue = "#61afef"
//!
//! [roles]
//! accent = "blue"
//!
//! [ui]
//! "mode.full" = { fg = "black", bg = "green", modifiers = ["bold"] }
//! border = "accent"
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use toml::Value;

/// Sections that hold element styles.
pub const SECTIONS: &[&str] = &["ui", "tool", "diff", "review", "markdown", "setup"];

/// Fields of a style table.
const STYLE_FIELDS: &[&str] = &["fg", "bg", "modifiers"];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeFile {
    pub preset: Option<String>,
    /// The user's own colour names.
    pub palette: BTreeMap<String, String>,
    /// Role name to colour.
    pub roles: BTreeMap<String, String>,
    /// Element styles by `section.key`, e.g. `ui.mode.full`.
    pub elements: BTreeMap<String, StyleSpec>,
}

/// One element's style. Colours are names, palette or role names, hex or indexes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleSpec {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub modifiers: Vec<String>,
}

impl ThemeFile {
    /// Load `path`. A missing file is an empty theme; other problems are warnings.
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        match fs::read_to_string(path) {
            Ok(src) => Self::parse(&src),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(e) => (
                Self::default(),
                vec![format!("cannot read {}: {e}", path.display())],
            ),
        }
    }

    /// Parse leniently: invalid entries are skipped with a warning, the rest is kept.
    pub fn parse(src: &str) -> (Self, Vec<String>) {
        let mut theme = Self::default();
        let mut warnings = Vec::new();
        let table: toml::Table = match toml::from_str(src) {
            Ok(table) => table,
            Err(e) => {
                let line = e
                    .span()
                    .map(|s| src[..s.start.min(src.len())].matches('\n').count() + 1);
                let at = line.map(|l| format!(" (line {l})")).unwrap_or_default();
                warnings.push(format!(
                    "not valid TOML{at}: {}; using the preset only",
                    e.message().trim()
                ));
                return (theme, warnings);
            }
        };
        for (key, value) in table {
            match key.as_str() {
                "preset" => match value {
                    Value::String(s) => theme.preset = Some(s),
                    other => warnings.push(format!(
                        "preset: expected a string, found {}",
                        other.type_str()
                    )),
                },
                "palette" | "roles" => {
                    let Some(entries) = section(&key, value, &mut warnings) else {
                        continue;
                    };
                    for (name, value) in entries {
                        match color(&value) {
                            Some(c) => {
                                let target = if key == "palette" {
                                    &mut theme.palette
                                } else {
                                    &mut theme.roles
                                };
                                target.insert(name, c);
                            }
                            None => warnings.push(format!(
                                "{key}.{name}: expected a colour string, found {}",
                                value.type_str()
                            )),
                        }
                    }
                }
                s if SECTIONS.contains(&s) => {
                    let Some(entries) = section(&key, value, &mut warnings) else {
                        continue;
                    };
                    for (name, value) in entries {
                        elements(
                            &format!("{key}.{name}"),
                            value,
                            &mut theme.elements,
                            &mut warnings,
                        );
                    }
                }
                _ => warnings.push(format!("unknown key `{key}`, ignored")),
            }
        }
        (theme, warnings)
    }

    /// Apply `other` on top of this theme: its preset, colours and elements win.
    pub fn overlay(&mut self, other: ThemeFile) {
        if other.preset.is_some() {
            self.preset = other.preset;
        }
        self.palette.extend(other.palette);
        self.roles.extend(other.roles);
        self.elements.extend(other.elements);
    }
}

fn section(key: &str, value: Value, warnings: &mut Vec<String>) -> Option<toml::Table> {
    match value {
        Value::Table(t) => Some(t),
        other => {
            warnings.push(format!(
                "{key}: expected a table, found {}",
                other.type_str()
            ));
            None
        }
    }
}

/// A colour string, or a 256-colour index written as a number.
fn color(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Integer(n) if (0..=255).contains(n) => Some(n.to_string()),
        _ => None,
    }
}

/// One element, or a group of them. Quoted (`"mode.full" = …`) and unquoted
/// (`mode.full = …`) dotted keys give the same element: a table that is not a style (it
/// has keys other than fg, bg and modifiers) is read as nested elements.
fn elements(
    key: &str,
    value: Value,
    out: &mut BTreeMap<String, StyleSpec>,
    warnings: &mut Vec<String>,
) {
    match value {
        Value::Table(t) if !t.keys().all(|k| STYLE_FIELDS.contains(&k.as_str())) => {
            for (name, value) in t {
                elements(&format!("{key}.{name}"), value, out, warnings);
            }
        }
        value => match style(value) {
            Ok(spec) => {
                out.insert(key.to_owned(), spec);
            }
            Err(problem) => warnings.push(format!("{key}: {problem}")),
        },
    }
}

/// A colour (the foreground) or `{ fg, bg, modifiers }`.
fn style(value: Value) -> Result<StyleSpec, String> {
    let field = |name: &str, value: Option<&Value>| match value {
        None => Ok(None),
        Some(v) => color(v)
            .map(Some)
            .ok_or_else(|| format!("{name}: expected a colour string, found {}", v.type_str())),
    };
    match value {
        Value::Table(t) => {
            let modifiers = match t.get("modifiers") {
                None => Vec::new(),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|m| match m {
                        Value::String(s) => Ok(s.clone()),
                        other => Err(format!(
                            "modifiers: expected strings, found {}",
                            other.type_str()
                        )),
                    })
                    .collect::<Result<_, _>>()?,
                Some(other) => {
                    return Err(format!(
                        "modifiers: expected a list, found {}",
                        other.type_str()
                    ));
                }
            };
            Ok(StyleSpec {
                fg: field("fg", t.get("fg"))?,
                bg: field("bg", t.get("bg"))?,
                modifiers,
            })
        }
        other => match color(&other) {
            Some(fg) => Ok(StyleSpec {
                fg: Some(fg),
                ..StyleSpec::default()
            }),
            None => Err(format!(
                "expected a colour or a {{ fg, bg, modifiers }} table, found {}",
                other.type_str()
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fg(c: &str) -> StyleSpec {
        StyleSpec {
            fg: Some(c.into()),
            ..StyleSpec::default()
        }
    }

    #[test]
    fn missing_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let (theme, warnings) = ThemeFile::load(&tmp.path().join("theme.toml"));
        assert_eq!(theme, ThemeFile::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn reads_every_form() {
        let (theme, warnings) = ThemeFile::parse(
            r##"
            preset = "light"
            [palette]
            blue = "#61afef"
            [roles]
            accent = "blue"
            muted = 244
            [ui]
            border = "accent"
            "mode.full" = { fg = "black", bg = "green", modifiers = ["bold"] }
            input.full = "blue"
            "border.help" = {}
            [markdown]
            "code.block" = 136
            "##,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(theme.preset.as_deref(), Some("light"));
        assert_eq!(theme.palette["blue"], "#61afef");
        assert_eq!(theme.roles["accent"], "blue");
        assert_eq!(theme.roles["muted"], "244");
        assert_eq!(theme.elements["ui.border"], fg("accent"));
        assert_eq!(
            theme.elements["ui.mode.full"],
            StyleSpec {
                fg: Some("black".into()),
                bg: Some("green".into()),
                modifiers: vec!["bold".into()],
            }
        );
        assert_eq!(
            theme.elements["ui.input.full"],
            fg("blue"),
            "unquoted dotted keys work too"
        );
        assert_eq!(theme.elements["ui.border.help"], StyleSpec::default());
        assert_eq!(theme.elements["markdown.code.block"], fg("136"));
    }

    #[test]
    fn bad_entries_are_skipped_with_warnings() {
        let (theme, warnings) = ThemeFile::parse(
            r#"
            preset = 3
            colours = "x"
            palette = "red"
            [roles]
            accent = true
            error = "red"
            [ui]
            border = ["red"]
            cursor = { fg = 1.5 }
            hint = { modifiers = "bold" }
            note = "yellow"
            "#,
        );
        assert_eq!(warnings.len(), 7, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("unknown key `colours`")));
        assert!(warnings.iter().any(|w| w.starts_with("roles.accent")));
        assert!(warnings.iter().any(|w| w.starts_with("ui.cursor: fg")));
        // The good entries are kept.
        assert_eq!(theme.roles["error"], "red");
        assert_eq!(theme.elements["ui.note"], fg("yellow"));
        assert!(!theme.elements.contains_key("ui.border"));
    }

    #[test]
    fn invalid_toml_is_one_warning() {
        let (theme, warnings) = ThemeFile::parse("[ui]\nborder = \n");
        assert_eq!(theme, ThemeFile::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("line 2"), "{warnings:?}");
    }

    #[test]
    fn overlay_replaces_per_key() {
        let (mut base, _) = ThemeFile::parse("[roles]\naccent = \"cyan\"\nerror = \"red\"");
        let (top, _) = ThemeFile::parse("preset = \"light\"\n[roles]\naccent = \"blue\"");
        base.overlay(top);
        assert_eq!(base.preset.as_deref(), Some("light"));
        assert_eq!(base.roles["accent"], "blue");
        assert_eq!(base.roles["error"], "red");
    }
}

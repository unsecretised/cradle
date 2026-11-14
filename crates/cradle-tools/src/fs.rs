//! File tools: read, list, search, and two-phase write/edit (prepare a diff, then apply).

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use globset::GlobBuilder;
use ignore::WalkBuilder;
use regex::Regex;
use similar::TextDiff;

use crate::guard::display;
use crate::{PathGuard, Resolved, ToolError, WriteClass};

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_READ_LINES: usize = 2000;
const MAX_LIST_ENTRIES: usize = 1000;
const MAX_SEARCH_MATCHES: usize = 200;
const MAX_MATCH_LINE_CHARS: usize = 300;

/// Read a text file with line numbers. `offset` is 1-based.
pub fn read_file(
    guard: &PathGuard,
    path: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String, ToolError> {
    let resolved = guard.resolve(path)?;
    if resolved.abs.is_dir() {
        return Err(ToolError::new(format!(
            "`{}` is a directory; use list_files",
            display(&resolved.rel)
        )));
    }
    let text = read_text(&resolved)?;

    let start = offset.unwrap_or(1).max(1);
    let limit = limit.unwrap_or(DEFAULT_READ_LINES).max(1);
    let total = text.lines().count();
    let mut out = String::new();
    for (i, line) in text.lines().enumerate().skip(start - 1).take(limit) {
        writeln!(out, "{:>6}\t{line}", i + 1).unwrap();
    }
    let shown_end = (start - 1 + limit).min(total);
    if total == 0 {
        out.push_str("(empty file)\n");
    } else if start > total {
        writeln!(
            out,
            "(offset {start} is past the end: file has {total} lines)"
        )
        .unwrap();
    } else if shown_end < total {
        writeln!(
            out,
            "(showing lines {start}-{shown_end} of {total}; use offset to read more)"
        )
        .unwrap();
    }
    Ok(out)
}

/// List files under `path` (default: the root), optionally filtered by a glob that is
/// matched against the path relative to the project root.
pub fn list_files(
    guard: &PathGuard,
    path: Option<&str>,
    glob: Option<&str>,
) -> Result<String, ToolError> {
    let start = match path {
        Some(p) => guard.resolve(p)?.abs,
        None => guard.root().to_path_buf(),
    };
    let matcher = glob
        .map(|g| {
            GlobBuilder::new(g)
                .literal_separator(false)
                .build()
                .map(|g| g.compile_matcher())
                .map_err(|e| ToolError::new(format!("invalid glob `{g}`: {e}")))
        })
        .transpose()?;

    let mut files = Vec::new();
    let mut truncated = false;
    for rel in walk(guard, &start) {
        if matcher.as_ref().is_none_or(|m| m.is_match(&rel)) {
            if files.len() == MAX_LIST_ENTRIES {
                truncated = true;
                break;
            }
            files.push(display(&rel));
        }
    }
    files.sort();

    let mut out = files.join("\n");
    if files.is_empty() {
        out.push_str("(no files)");
    }
    if truncated {
        write!(
            out,
            "\n(stopped at {MAX_LIST_ENTRIES} files; narrow the path or glob)"
        )
        .unwrap();
    }
    Ok(out)
}

/// Search file contents with a regex. Returns `path:line: text` entries.
pub fn search(
    guard: &PathGuard,
    pattern: &str,
    path: Option<&str>,
    glob: Option<&str>,
) -> Result<String, ToolError> {
    let regex = Regex::new(pattern).map_err(|e| ToolError::new(format!("invalid regex: {e}")))?;
    let start = match path {
        Some(p) => guard.resolve(p)?.abs,
        None => guard.root().to_path_buf(),
    };
    let matcher = glob
        .map(|g| {
            GlobBuilder::new(g)
                .literal_separator(false)
                .build()
                .map(|g| g.compile_matcher())
                .map_err(|e| ToolError::new(format!("invalid glob `{g}`: {e}")))
        })
        .transpose()?;

    let mut out = String::new();
    let mut count = 0;
    'files: for rel in walk(guard, &start) {
        if matcher.as_ref().is_some_and(|m| !m.is_match(&rel)) {
            continue;
        }
        let abs = guard.root().join(&rel);
        let Ok(meta) = fs::metadata(&abs) else {
            continue;
        };
        if meta.len() > MAX_FILE_BYTES {
            continue;
        }
        let Ok(text) = fs::read_to_string(&abs) else {
            continue;
        };
        for (i, line) in text.lines().enumerate() {
            if regex.is_match(line) {
                if count == MAX_SEARCH_MATCHES {
                    write!(
                        out,
                        "(stopped at {MAX_SEARCH_MATCHES} matches; narrow the search)"
                    )
                    .unwrap();
                    break 'files;
                }
                let line: String = line.chars().take(MAX_MATCH_LINE_CHARS).collect();
                writeln!(out, "{}:{}: {line}", display(&rel), i + 1).unwrap();
                count += 1;
            }
        }
    }
    if count == 0 {
        out.push_str("(no matches)");
    }
    Ok(out)
}

/// A write that has been checked and diffed but not yet applied.
#[derive(Debug, Clone)]
pub struct PreparedWrite {
    pub target: Resolved,
    pub class: WriteClass,
    pub existed: bool,
    pub new_content: String,
    /// Unified diff from the old content (empty if the file is new) to the new.
    pub diff: String,
    pub added: usize,
    pub removed: usize,
}

/// Prepare replacing (or creating) a whole file.
pub fn prepare_write(
    guard: &PathGuard,
    path: &str,
    content: &str,
) -> Result<PreparedWrite, ToolError> {
    let (target, class) = guard.resolve_write(path)?;
    if target.abs.is_dir() {
        return Err(ToolError::new(format!(
            "`{}` is a directory",
            display(&target.rel)
        )));
    }
    let old = if target.abs.exists() {
        Some(read_text(&target)?)
    } else {
        None
    };
    Ok(prepared(target, class, old.as_deref(), content.to_owned()))
}

/// Like [`prepare_write`], but may also create project config that does not exist yet
/// (for `/scaffold`).
pub fn prepare_scaffold_write(
    guard: &PathGuard,
    path: &str,
    content: &str,
) -> Result<PreparedWrite, ToolError> {
    let (target, class) = guard.resolve_scaffold(path)?;
    if class != WriteClass::AgentsConfig {
        return prepare_write(guard, path, content);
    }
    Ok(prepared(target, class, None, content.to_owned()))
}

/// Prepare an exact-string replacement in an existing file.
pub fn prepare_edit(
    guard: &PathGuard,
    path: &str,
    old_string: &str,
    new_string: &str,
    replace_all: bool,
) -> Result<PreparedWrite, ToolError> {
    let (target, class) = guard.resolve_write(path)?;
    if !target.abs.is_file() {
        return Err(ToolError::new(format!(
            "`{}` does not exist; use write_file to create it",
            display(&target.rel)
        )));
    }
    if old_string.is_empty() {
        return Err(ToolError::new("old_string must not be empty"));
    }
    if old_string == new_string {
        return Err(ToolError::new("old_string and new_string are identical"));
    }
    let old = read_text(&target)?;
    let new = match old.matches(old_string).count() {
        0 => {
            return Err(ToolError::new(
                "old_string not found; read the file and copy the text exactly",
            ));
        }
        1 => old.replacen(old_string, new_string, 1),
        _ if replace_all => old.replace(old_string, new_string),
        n => {
            return Err(ToolError::new(format!(
                "old_string occurs {n} times; add surrounding context or set replace_all"
            )));
        }
    };
    Ok(prepared(target, class, Some(&old), new))
}

/// Write a prepared change to disk.
pub fn apply(write: &PreparedWrite) -> Result<String, ToolError> {
    if let Some(parent) = write.target.abs.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| ToolError::new(format!("cannot create directory: {e}")))?;
    }
    fs::write(&write.target.abs, &write.new_content).map_err(|e| {
        ToolError::new(format!(
            "cannot write `{}`: {e}",
            display(&write.target.rel)
        ))
    })?;
    let verb = if write.existed { "Updated" } else { "Created" };
    Ok(format!(
        "{verb} {} (+{} -{})",
        display(&write.target.rel),
        write.added,
        write.removed
    ))
}

fn prepared(
    target: Resolved,
    class: WriteClass,
    old: Option<&str>,
    new_content: String,
) -> PreparedWrite {
    let diff = unified_diff(&display(&target.rel), old, &new_content);
    let (mut added, mut removed) = (0, 0);
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
        }
    }
    PreparedWrite {
        target,
        class,
        existed: old.is_some(),
        new_content,
        diff,
        added,
        removed,
    }
}

/// A unified diff of `name` from `old` (`None`: the file did not exist) to `new`.
pub fn unified_diff(name: &str, old: Option<&str>, new: &str) -> String {
    let from = match old {
        Some(_) => format!("a/{name}"),
        None => "/dev/null".to_owned(),
    };
    TextDiff::from_lines(old.unwrap_or(""), new)
        .unified_diff()
        .context_radius(3)
        .header(&from, &format!("b/{name}"))
        .to_string()
}

fn read_text(resolved: &Resolved) -> Result<String, ToolError> {
    let name = display(&resolved.rel);
    let meta = fs::metadata(&resolved.abs)
        .map_err(|e| ToolError::new(format!("cannot read `{name}`: {e}")))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(ToolError::new(format!(
            "`{name}` is larger than {} MB",
            MAX_FILE_BYTES / 1024 / 1024
        )));
    }
    let bytes = fs::read(&resolved.abs)
        .map_err(|e| ToolError::new(format!("cannot read `{name}`: {e}")))?;
    String::from_utf8(bytes).map_err(|_| ToolError::new(format!("`{name}` is not UTF-8 text")))
}

/// Walk files under `start`, honouring nested `.gitignore` files and the guard's rules.
/// Yields paths relative to the root.
fn walk<'a>(guard: &'a PathGuard, start: &Path) -> impl Iterator<Item = std::path::PathBuf> + 'a {
    let root = guard.root().to_path_buf();
    WalkBuilder::new(start)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(move |e| e.path().strip_prefix(&root).ok().map(Path::to_path_buf))
        .filter(move |rel| !guard.is_hidden(rel, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathGuard) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src/nested")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {\n    hello();\n}\n").unwrap();
        fs::write(root.join("src/nested/lib.rs"), "pub fn hello() {}\n").unwrap();
        fs::write(root.join("target/out.rs"), "fn hello() {}\n").unwrap();
        fs::write(root.join(".env"), "KEY=hello").unwrap();
        let guard = PathGuard::new(root).unwrap();
        (tmp, guard)
    }

    #[test]
    fn reads_with_line_numbers_and_ranges() {
        let (_tmp, guard) = project();
        let out = read_file(&guard, "src/main.rs", None, None).unwrap();
        assert!(out.starts_with("     1\tfn main() {\n"));
        let out = read_file(&guard, "src/main.rs", Some(2), Some(1)).unwrap();
        assert!(out.contains("     2\t    hello();"));
        assert!(out.contains("showing lines 2-2 of 3"));
    }

    #[test]
    fn lists_and_searches_without_hidden_files() {
        let (_tmp, guard) = project();
        let listed = list_files(&guard, None, None).unwrap();
        assert!(listed.contains("src/main.rs") && listed.contains("src/nested/lib.rs"));
        assert!(
            !listed.contains("target") && !listed.contains(".env"),
            "{listed}"
        );
        assert_eq!(
            list_files(&guard, None, Some("**/lib.rs")).unwrap(),
            "src/nested/lib.rs"
        );

        let found = search(&guard, "hello", None, None).unwrap();
        assert!(found.contains("src/main.rs:2:") && found.contains("src/nested/lib.rs:1:"));
        assert!(
            !found.contains("target") && !found.contains(".env"),
            "{found}"
        );
    }

    #[test]
    fn edit_requires_unique_match() {
        let (_tmp, guard) = project();
        let edit = prepare_edit(&guard, "src/main.rs", "hello()", "goodbye()", false).unwrap();
        assert_eq!((edit.added, edit.removed), (1, 1));
        assert!(edit.diff.contains("+    goodbye();"));
        apply(&edit).unwrap();
        assert!(
            read_file(&guard, "src/main.rs", None, None)
                .unwrap()
                .contains("goodbye")
        );

        assert!(prepare_edit(&guard, "src/main.rs", "nope", "x", false).is_err());
        fs::write(guard.root().join("src/dup.rs"), "a a").unwrap();
        assert!(prepare_edit(&guard, "src/dup.rs", "a", "b", false).is_err());
        let all = prepare_edit(&guard, "src/dup.rs", "a", "b", true).unwrap();
        assert_eq!(all.new_content, "b b");
    }

    #[test]
    fn write_creates_files_and_classifies() {
        let (_tmp, guard) = project();
        let w = prepare_write(&guard, "src/new/mod.rs", "x\n").unwrap();
        assert!(!w.existed && w.class == WriteClass::Normal);
        assert_eq!(apply(&w).unwrap(), "Created src/new/mod.rs (+1 -0)");
        assert!(matches!(
            prepare_write(&guard, "build.rs", "").unwrap().class,
            WriteClass::Protected(_)
        ));
        assert!(prepare_write(&guard, ".env", "x").is_err());
    }
}

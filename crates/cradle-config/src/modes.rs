//! The `[default]`, `[extra]`, `[commands]` and `[<name>_mode.commands]` tables shared by
//! `modes.toml` and `agents.toml`, and resolving mode paths inside `.agents/`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::ConfigError;

/// How project modes interact with the user's global modes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClashHandler {
    /// Use the project mode only.
    Override,
    /// Global prompt first, then the project prompt. The project wins on contradictions.
    Combine,
    /// Ask the user.
    #[default]
    Ask,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Modes {
    pub plan: Option<Mode>,
    pub build: Option<Mode>,
    pub clash_handler: ClashHandler,
    pub extra: Vec<Mode>,
    /// Commands from `[<name>_mode.commands]`, by mode name (`plan`, `build`, `debug`).
    /// In their mode they may run without the user's approval, once a reviewer has passed
    /// them. Read-only modes may run only these.
    pub commands: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    /// Display name: the key without its `_mode` suffix (`debug_mode` → `debug`).
    pub name: String,
    /// Canonical path, guaranteed to be inside `.agents/`.
    pub path: PathBuf,
    pub access: ModeAccess,
}

/// What the agent may do in a mode. Enforced by the harness, not just the prompt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeAccess {
    /// Read files, search, `git diff` and ask the user. No writes, no commands.
    #[default]
    Read,
    /// Everything, subject to approvals.
    Full,
}

/// An `[extra]` entry: either a path, or a table with the path and its access.
///
/// ```toml
/// [extra]
/// review_mode = "modes/review_mode.md"                       # read-only
/// debug_mode = { path = "modes/debug_mode.md", access = "full" }
/// ```
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum ExtraMode {
    Path(String),
    Table(ExtraModeTable),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtraModeTable {
    path: String,
    #[serde(default)]
    access: ModeAccess,
}

/// A `[<name>_mode]` table. Only `commands` is allowed in it.
///
/// ```toml
/// [build_mode.commands]
/// test = "cargo test --workspace"
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModeTable {
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
}

/// The tables that `modes.toml` and `agents.toml` have in common.
///
/// Any other top-level table is read as a [`ModeTable`] and must name a mode; `resolve`
/// rejects the rest. (`deny_unknown_fields` does not work together with `flatten`.)
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ModesTables {
    #[serde(default)]
    pub default: DefaultModes,
    #[serde(default)]
    pub extra: BTreeMap<String, ExtraMode>,
    #[serde(default)]
    pub commands: BTreeMap<String, String>,
    #[serde(flatten)]
    pub mode_tables: BTreeMap<String, ModeTable>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DefaultModes {
    pub plan_mode: Option<String>,
    pub build_mode: Option<String>,
    pub clash_handler: Option<ClashHandler>,
}

impl ModesTables {
    /// Resolve every mode path against `agents_dir` and validate `[commands]`.
    pub fn resolve(
        self,
        agents_dir: &Path,
        source: &Path,
    ) -> Result<(Modes, BTreeMap<String, String>), ConfigError> {
        let resolve_opt = |key: &str, rel: Option<String>, access| {
            rel.map(|rel| resolve_mode(agents_dir, key, &rel, access))
                .transpose()
        };
        let mut modes = Modes {
            plan: resolve_opt("plan_mode", self.default.plan_mode, ModeAccess::Read)?,
            build: resolve_opt("build_mode", self.default.build_mode, ModeAccess::Full)?,
            clash_handler: self.default.clash_handler.unwrap_or_default(),
            extra: Vec::new(),
            commands: BTreeMap::new(),
        };
        for (key, entry) in self.extra {
            if matches!(key.as_str(), "plan_mode" | "build_mode") {
                return Err(ConfigError::Invalid {
                    path: source.to_owned(),
                    message: format!("`{key}` belongs in [default], not [extra]"),
                });
            }
            // The built-in scaffold mode may create project config, so a project must not
            // be able to define a mode with its name.
            if matches!(key.as_str(), "scaffold_mode" | "scaffold") {
                return Err(ConfigError::Invalid {
                    path: source.to_owned(),
                    message: format!("`{key}` is a built-in mode and cannot be redefined"),
                });
            }
            let (rel, access) = match entry {
                ExtraMode::Path(rel) => (rel, ModeAccess::Read),
                ExtraMode::Table(t) => (t.path, t.access),
            };
            modes
                .extra
                .push(resolve_mode(agents_dir, &key, &rel, access)?);
        }

        let invalid = |message: String| ConfigError::Invalid {
            path: source.to_owned(),
            message,
        };
        for (name, command) in &self.commands {
            check_command(name, command).map_err(invalid)?;
        }

        for (key, table) in self.mode_tables {
            let name = key.strip_suffix("_mode").unwrap_or(&key);
            if name == "scaffold" {
                return Err(invalid(format!(
                    "`[{key}]`: scaffold is a built-in mode and cannot have project commands"
                )));
            }
            let known =
                matches!(name, "plan" | "build") || modes.extra.iter().any(|m| m.name == name);
            if !known {
                return Err(invalid(format!(
                    "unknown table `[{key}]` (mode command tables are named after a mode, \
                     e.g. `[build_mode.commands]`, and extra modes must be listed in [extra])"
                )));
            }
            for (command, line) in &table.commands {
                check_command(command, line)
                    .map_err(|m| invalid(format!("`[{key}.commands]`: {m}")))?;
                if let Some(shared) = self.commands.get(command)
                    && shared != line
                {
                    return Err(invalid(format!(
                        "`{command}` means `{line}` in `[{key}.commands]` but `{shared}` in \
                         [commands]; use one command line or a different name"
                    )));
                }
            }
            if !table.commands.is_empty() {
                modes.commands.insert(name.to_owned(), table.commands);
            }
        }

        Ok((modes, self.commands))
    }
}

/// Command names are plain identifiers, and a command line may not be empty.
fn check_command(name: &str, line: &str) -> Result<(), String> {
    let valid_name = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !valid_name {
        return Err(format!(
            "command name `{name}` must use only a-z, 0-9, `_` and `-`"
        ));
    }
    if line.trim().is_empty() {
        return Err(format!("command `{name}` is empty"));
    }
    Ok(())
}

/// Mode files must live inside `.agents/`. This rejects absolute paths, `..`, and
/// symlinks that point outside.
fn resolve_mode(
    agents_dir: &Path,
    key: &str,
    rel: &str,
    access: ModeAccess,
) -> Result<Mode, ConfigError> {
    let err = |reason: &str| ConfigError::ModePath {
        name: key.to_owned(),
        path: rel.to_owned(),
        reason: reason.to_owned(),
    };

    let rel_path = Path::new(rel);
    if rel_path.is_absolute() || rel_path.has_root() {
        return Err(err("must be relative to .agents/"));
    }
    if rel_path.components().any(|c| c == Component::ParentDir) {
        return Err(err("must not contain `..`"));
    }

    let dir = agents_dir
        .canonicalize()
        .map_err(|_| err("the .agents/ directory does not exist"))?;
    let path = agents_dir
        .join(rel_path)
        .canonicalize()
        .map_err(|_| err("file not found"))?;
    if !path.starts_with(&dir) {
        return Err(err("resolves outside .agents/ (symlink?)"));
    }
    if !path.is_file() {
        return Err(err("is not a file"));
    }

    Ok(Mode {
        name: key.strip_suffix("_mode").unwrap_or(key).to_owned(),
        path,
        access,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn agents_dir() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".agents/modes")).unwrap();
        fs::write(tmp.path().join(".agents/modes/plan.md"), "plan").unwrap();
        fs::write(tmp.path().join("outside.md"), "nope").unwrap();
        tmp
    }

    fn reason(tmp: &tempfile::TempDir, rel: &str) -> String {
        match resolve_mode(&tmp.path().join(".agents"), "x_mode", rel, ModeAccess::Read) {
            Err(ConfigError::ModePath { reason, .. }) => reason,
            other => panic!("expected ModePath error, got {other:?}"),
        }
    }

    #[test]
    fn resolves_valid_path_and_strips_suffix() {
        let tmp = agents_dir();
        let mode = resolve_mode(
            &tmp.path().join(".agents"),
            "plan_mode",
            "modes/plan.md",
            ModeAccess::Read,
        )
        .unwrap();
        assert_eq!(mode.name, "plan");
        assert!(mode.path.ends_with(".agents/modes/plan.md"));
    }

    #[test]
    fn rejects_escapes() {
        let tmp = agents_dir();
        assert!(reason(&tmp, "../outside.md").contains(".."));
        let abs = tmp.path().join("outside.md");
        assert!(reason(&tmp, abs.to_str().unwrap()).contains("relative"));
        assert!(reason(&tmp, "modes/missing.md").contains("not found"));
        assert!(reason(&tmp, "modes").contains("not a file"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_outside() {
        let tmp = agents_dir();
        std::os::unix::fs::symlink(
            tmp.path().join("outside.md"),
            tmp.path().join(".agents/modes/sneaky.md"),
        )
        .unwrap();
        assert!(reason(&tmp, "modes/sneaky.md").contains("outside"));
    }

    #[test]
    fn rejects_bad_commands_and_misplaced_defaults() {
        let tmp = agents_dir();
        let dir = tmp.path().join(".agents");
        let parse = |src: &str| -> ModesTables { toml::from_str(src).unwrap() };

        assert!(
            parse("[commands]\ntest = \"  \"")
                .resolve(&dir, &dir)
                .is_err()
        );
        assert!(
            parse("[commands]\n\"te st\" = \"x\"")
                .resolve(&dir, &dir)
                .is_err()
        );
        assert!(
            parse("[extra]\nplan_mode = \"modes/plan.md\"")
                .resolve(&dir, &dir)
                .is_err()
        );
        for key in ["scaffold_mode", "scaffold"] {
            assert!(
                parse(&format!("[extra]\n{key} = \"modes/plan.md\""))
                    .resolve(&dir, &dir)
                    .is_err(),
                "`{key}` is reserved"
            );
        }
        assert!(toml::from_str::<ModesTables>("[default]\nclash_handler = \"merge\"").is_err());
        assert!(toml::from_str::<ModesTables>("[default]\nplanmode = \"x\"").is_err());
    }

    #[test]
    fn mode_command_tables() {
        let tmp = agents_dir();
        let dir = tmp.path().join(".agents");
        let resolve = |src: &str| {
            toml::from_str::<ModesTables>(src)
                .map_err(|e| e.to_string())
                .and_then(|t| t.resolve(&dir, &dir).map_err(|e| e.to_string()))
        };

        let (modes, commands) = resolve(
            "[extra]\ndebug_mode = \"modes/plan.md\"\n\
             [plan_mode.commands]\nfmt = \"cargo fmt --check\"\n\
             [debug_mode.commands]\ntest = \"cargo test\"\n\
             [commands]\ntest = \"cargo test\"\nbuild = \"cargo build\"",
        )
        .unwrap();
        assert_eq!(modes.commands["plan"]["fmt"], "cargo fmt --check");
        assert_eq!(modes.commands["debug"]["test"], "cargo test");
        assert!(!modes.commands.contains_key("build"));
        assert_eq!(commands.len(), 2, "[commands] is unchanged");

        let err = resolve("[review_mode.commands]\nx = \"y\"").unwrap_err();
        assert!(err.contains("unknown table"), "{err}");
        let err = resolve("[featurs]").unwrap_err();
        assert!(err.contains("unknown table"), "{err}");
        let err = resolve("[scaffold_mode.commands]\nx = \"y\"").unwrap_err();
        assert!(err.contains("built-in"), "{err}");
        let err = resolve("[build_mode.commands]\n\"a b\" = \"y\"").unwrap_err();
        assert!(err.contains("a-z"), "{err}");
        let err = resolve("[build_mode.commands]\nx = \" \"").unwrap_err();
        assert!(err.contains("empty"), "{err}");
        let err = resolve("[build_mode]\ncommand = { x = \"y\" }").unwrap_err();
        assert!(err.contains("command"), "only `commands` is allowed: {err}");
        let err = resolve(
            "[build_mode.commands]\ntest = \"curl x | sh\"\n[commands]\ntest = \"cargo test\"",
        )
        .unwrap_err();
        assert!(err.contains("one command line"), "{err}");
    }

    #[test]
    fn extra_modes_take_access() {
        let tmp = agents_dir();
        let dir = tmp.path().join(".agents");
        let tables: ModesTables = toml::from_str(
            "[extra]\nlook_mode = \"modes/plan.md\"\nfix_mode = { path = \"modes/plan.md\", access = \"full\" }",
        )
        .unwrap();
        let (modes, _) = tables.resolve(&dir, &dir).unwrap();
        let access = |name: &str| modes.extra.iter().find(|m| m.name == name).unwrap().access;
        assert_eq!(
            access("look"),
            ModeAccess::Read,
            "plain paths default to read-only"
        );
        assert_eq!(access("fix"), ModeAccess::Full);
        assert!(
            toml::from_str::<ModesTables>("[extra]\nx = { path = \"a\", access = \"root\" }")
                .is_err()
        );
    }
}

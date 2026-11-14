//! The single-file `agents.toml` form for small projects.
//!
//! ```toml
//! [docs]
//! product = "A CLI that does abcd"
//! rules = "Never commit."
//!
//! [features]
//! "abcd" = "allows user to do abcd"
//!
//! [roadmap]
//! "xyz" = "let the user do xyz"
//!
//! [architecture.src]
//! "main.rs" = "entrypoint"
//!
//! [default]
//! plan_mode = "modes/plan_mode.md"   # still resolved inside .agents/
//!
//! [commands]
//! test = "cargo test"
//!
//! [build_mode.commands]              # run without approval in build mode, once reviewed
//! check = "cargo check"
//! ```

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::modes::{DefaultModes, ExtraMode, ModeTable, ModesTables};
use crate::{
    AGENTS_DIR, AGENTS_TOML, ConfigError, Docs, Entry, EntryBody, ProjectConfig, SourceKind,
};

/// Unknown top-level tables land in `mode_tables`, where `ModesTables::resolve` rejects
/// any that do not name a mode.
#[derive(Debug, Default, Deserialize)]
struct AgentsToml {
    #[serde(default)]
    docs: TomlDocs,
    #[serde(default)]
    features: BTreeMap<String, String>,
    #[serde(default)]
    roadmap: BTreeMap<String, String>,
    #[serde(default)]
    bugfixes: BTreeMap<String, String>,
    #[serde(default)]
    architecture: toml::Table,
    #[serde(default)]
    default: DefaultModes,
    #[serde(default)]
    extra: BTreeMap<String, ExtraMode>,
    #[serde(default)]
    commands: BTreeMap<String, String>,
    #[serde(flatten)]
    mode_tables: BTreeMap<String, ModeTable>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TomlDocs {
    product: Option<String>,
    rules: Option<String>,
    build: Option<String>,
    tests: Option<String>,
    deps: Option<String>,
}

pub(crate) fn load(root: &Path) -> Result<ProjectConfig, ConfigError> {
    let path = root.join(AGENTS_TOML);
    let src = fs::read_to_string(&path).map_err(|e| ConfigError::io(&path, e))?;
    let parsed: AgentsToml = toml::from_str(&src).map_err(|source| ConfigError::Toml {
        path: path.clone(),
        source,
    })?;

    let tables = ModesTables {
        default: parsed.default,
        extra: parsed.extra,
        commands: parsed.commands,
        mode_tables: parsed.mode_tables,
    };
    let (modes, commands) = tables.resolve(&root.join(AGENTS_DIR), &path)?;

    let architecture = if parsed.architecture.is_empty() {
        None
    } else {
        let mut out = String::new();
        render_tree(&parsed.architecture, 0, &mut out);
        Some(out)
    };
    let features_overview = (!parsed.features.is_empty()).then(|| {
        parsed
            .features
            .iter()
            .map(|(name, summary)| format!("- {name}: {summary}\n"))
            .collect()
    });

    Ok(ProjectConfig {
        root: root.to_owned(),
        source: SourceKind::File,
        docs: Docs {
            product: parsed.docs.product,
            rules: parsed.docs.rules,
            architecture,
            features_overview,
            build: parsed.docs.build,
            tests: parsed.docs.tests,
            deps: parsed.docs.deps,
        },
        features: inline_entries(parsed.features),
        roadmap: inline_entries(parsed.roadmap),
        bugfixes: inline_entries(parsed.bugfixes),
        modes,
        commands,
        skills: crate::skills::list(&root.join(AGENTS_DIR).join("skills"))?,
    })
}

fn inline_entries(map: BTreeMap<String, String>) -> Vec<Entry> {
    map.into_iter()
        .map(|(name, summary)| Entry {
            name,
            body: EntryBody::Inline(summary),
        })
        .collect()
}

/// Render `[architecture.*]` tables as an indented file tree. Files come before
/// subdirectories.
fn render_tree(table: &toml::Table, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    let (dirs, files): (Vec<_>, Vec<_>) = table.iter().partition(|(_, v)| v.is_table());
    for (name, value) in files {
        match value.as_str() {
            Some(desc) => writeln!(out, "{indent}{name} — {desc}").unwrap(),
            None => writeln!(out, "{indent}{name} — {value}").unwrap(),
        }
    }
    for (name, value) in dirs {
        writeln!(out, "{indent}{name}/").unwrap();
        render_tree(value.as_table().unwrap(), depth + 1, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_example_from_spec() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join(".agents/modes")).unwrap();
        fs::write(tmp.path().join(".agents/modes/plan_mode.md"), "plan").unwrap();
        fs::write(
            tmp.path().join("agents.toml"),
            r#"
            [docs]
            product = "Splits and parses code"

            [features]
            "abcd" = "allows user to do abcd"

            [roadmap]
            "xyz" = "let the user do xyz"

            [architecture.src]
            "main.rs" = "entrypoint"
            "module.rs" = "modules"

            [architecture.src.module]
            "spliter.rs" = "split code"
            "parser.rs" = "parse code"

            [default]
            plan_mode = "modes/plan_mode.md"
            clash_handler = "combine"

            [commands]
            test = "cargo test"

            [plan_mode.commands]
            check = "cargo check"
            "#,
        )
        .unwrap();

        let config = load(tmp.path()).unwrap();
        assert_eq!(config.source, SourceKind::File);
        assert_eq!(
            config.docs.architecture.as_deref(),
            Some(
                "src/\n  main.rs — entrypoint\n  module.rs — modules\n  module/\n    \
                 parser.rs — parse code\n    spliter.rs — split code\n"
            )
        );
        assert_eq!(
            config.features,
            [Entry {
                name: "abcd".into(),
                body: EntryBody::Inline("allows user to do abcd".into())
            }]
        );
        assert_eq!(config.roadmap.len(), 1);
        assert_eq!(config.modes.plan.as_ref().unwrap().name, "plan");
        assert_eq!(config.modes.clash_handler, crate::ClashHandler::Combine);
        assert_eq!(config.commands["test"], "cargo test");
        assert_eq!(config.modes.commands["plan"]["check"], "cargo check");
    }

    #[test]
    fn rejects_unknown_tables() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("agents.toml"), "[featurs]\na = \"b\"").unwrap();
        assert!(matches!(load(tmp.path()), Err(ConfigError::Toml { .. })));
        fs::write(tmp.path().join("agents.toml"), "[featurs]").unwrap();
        assert!(matches!(load(tmp.path()), Err(ConfigError::Invalid { .. })));
    }
}

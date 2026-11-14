//! Project and user configuration for Cradle.
//!
//! A project describes itself either with an `.agents/` directory or a single `agents.toml`.
//! Both forms load into the same [`ProjectConfig`]. See
//! `.agents/roadmap/FEATURE_CONFIG_LOADING.md`.

mod agents_toml;
mod directory;
mod error;
mod modes;
pub mod paths;
pub mod user;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use error::ConfigError;
pub use modes::{ClashHandler, Mode, ModeAccess, Modes};

pub const AGENTS_DIR: &str = ".agents";
pub const AGENTS_TOML: &str = "agents.toml";

/// Which form a project's config is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// The `.agents/` directory.
    Directory,
    /// A single `agents.toml` at the project root.
    File,
}

/// What [`detect`] found in a project root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detection {
    None,
    One(SourceKind),
    /// Both forms exist. The user has to choose one.
    Both,
}

/// Everything Cradle knows about a project, independent of the form it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub root: PathBuf,
    pub source: SourceKind,
    pub docs: Docs,
    pub features: Vec<Entry>,
    pub roadmap: Vec<Entry>,
    pub bugfixes: Vec<Entry>,
    pub modes: Modes,
    /// The only commands the agent may run, by name.
    pub commands: BTreeMap<String, String>,
}

/// The single-document sections. All optional: a project may leave any of them out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Docs {
    pub product: Option<String>,
    pub rules: Option<String>,
    pub architecture: Option<String>,
    pub features_overview: Option<String>,
    pub build: Option<String>,
    pub tests: Option<String>,
    pub deps: Option<String>,
}

/// One feature, roadmap item or bugfix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub body: EntryBody,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryBody {
    /// A markdown file in `.agents/`, read on demand.
    File(PathBuf),
    /// A one-line description from `agents.toml`.
    Inline(String),
}

/// Look for `.agents/` and `agents.toml` in `root`.
///
/// `.agents/` only counts as the directory form if it holds something other than `modes/`,
/// because projects using `agents.toml` keep their mode files in `.agents/modes/` too.
pub fn detect(root: &Path) -> Result<Detection, ConfigError> {
    let file = root.join(AGENTS_TOML).is_file();
    let dir = directory::is_directory_form(&root.join(AGENTS_DIR))?;
    Ok(match (dir, file) {
        (false, false) => Detection::None,
        (true, false) => Detection::One(SourceKind::Directory),
        (false, true) => Detection::One(SourceKind::File),
        (true, true) => Detection::Both,
    })
}

/// Load the project config in the given form.
pub fn load(root: &Path, source: SourceKind) -> Result<ProjectConfig, ConfigError> {
    match source {
        SourceKind::Directory => directory::load(root),
        SourceKind::File => agents_toml::load(root),
    }
}

/// Detect and load. Fails with [`ConfigError::Clash`] if both forms exist, so the caller
/// can ask the user and call [`load`] with their choice.
pub fn load_detected(root: &Path) -> Result<ProjectConfig, ConfigError> {
    match detect(root)? {
        Detection::None => Err(ConfigError::NotFound(root.to_owned())),
        Detection::Both => Err(ConfigError::Clash(root.to_owned())),
        Detection::One(source) => load(root, source),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn detects_each_form() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert_eq!(detect(root).unwrap(), Detection::None);

        fs::create_dir_all(root.join(".agents/modes")).unwrap();
        assert_eq!(
            detect(root).unwrap(),
            Detection::None,
            "a bare .agents/modes/ is not the directory form"
        );

        fs::write(root.join("agents.toml"), "").unwrap();
        assert_eq!(detect(root).unwrap(), Detection::One(SourceKind::File));

        fs::write(root.join(".agents/product.md"), "p").unwrap();
        assert_eq!(detect(root).unwrap(), Detection::Both);
        assert!(matches!(load_detected(root), Err(ConfigError::Clash(_))));

        fs::remove_file(root.join("agents.toml")).unwrap();
        assert_eq!(detect(root).unwrap(), Detection::One(SourceKind::Directory));
    }

    #[test]
    fn loads_this_repository() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let config = load_detected(&root).unwrap();
        assert_eq!(config.source, SourceKind::Directory);
        assert!(config.docs.rules.is_some());
        assert!(config.commands.contains_key("test"));
        assert!(config.modes.plan.is_some() && config.modes.build.is_some());
        assert!(config.modes.extra.iter().any(|m| m.name == "debug"));
    }
}

//! The `.agents/` directory form.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::modes::ModesTables;
use crate::{AGENTS_DIR, ConfigError, Docs, Entry, EntryBody, ProjectConfig, SourceKind};

pub(crate) fn is_directory_form(dir: &Path) -> Result<bool, ConfigError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            return Ok(false);
        }
        Err(e) => return Err(ConfigError::io(dir, e)),
    };
    for entry in entries {
        let entry = entry.map_err(|e| ConfigError::io(dir, e))?;
        if entry.file_name() != "modes" {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn load(root: &Path) -> Result<ProjectConfig, ConfigError> {
    let dir = root.join(AGENTS_DIR);

    let modes_path = dir.join("modes.toml");
    let tables: ModesTables = match read_optional(&modes_path)? {
        Some(src) => toml::from_str(&src).map_err(|source| ConfigError::Toml {
            path: modes_path.clone(),
            source,
        })?,
        None => ModesTables::default(),
    };
    let (modes, commands) = tables.resolve(&dir, &modes_path)?;

    Ok(ProjectConfig {
        root: root.to_owned(),
        source: SourceKind::Directory,
        docs: Docs {
            product: read_optional(&dir.join("product.md"))?,
            rules: read_optional(&dir.join("RULES.md"))?,
            architecture: read_optional(&dir.join("architecture.md"))?,
            features_overview: read_optional(&dir.join("features.md"))?,
            build: read_optional(&dir.join("BUILD.md"))?,
            tests: read_optional(&dir.join("TESTS.md"))?,
            deps: read_optional(&dir.join("deps.md"))?,
        },
        features: list_entries(&dir.join("features"))?,
        roadmap: list_entries(&dir.join("roadmap"))?,
        bugfixes: list_entries(&dir.join("bugfixes"))?,
        modes,
        commands,
    })
}

fn read_optional(path: &Path) -> Result<Option<String>, ConfigError> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ConfigError::io(path, e)),
    }
}

/// The `*.md` files in a section directory, sorted by name. Contents are read on demand.
fn list_entries(dir: &Path) -> Result<Vec<Entry>, ConfigError> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ConfigError::io(dir, e)),
    };
    let mut entries = Vec::new();
    for item in read {
        let path = item.map_err(|e| ConfigError::io(dir, e))?.path();
        if path.is_file()
            && path.extension().is_some_and(|ext| ext == "md")
            && let Some(name) = path.file_stem().and_then(|s| s.to_str())
        {
            entries.push(Entry {
                name: name.to_owned(),
                body: EntryBody::File(path.clone()),
            });
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_minimal_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".agents");
        fs::create_dir_all(dir.join("roadmap")).unwrap();
        fs::write(dir.join("product.md"), "Product").unwrap();
        fs::write(dir.join("roadmap/FEATURE_B.md"), "b").unwrap();
        fs::write(dir.join("roadmap/FEATURE_A.md"), "a").unwrap();
        fs::write(dir.join("roadmap/notes.txt"), "ignored").unwrap();

        let config = load(tmp.path()).unwrap();
        assert_eq!(config.docs.product.as_deref(), Some("Product"));
        assert_eq!(config.docs.rules, None);
        let names: Vec<_> = config.roadmap.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["FEATURE_A", "FEATURE_B"]);
        assert!(config.features.is_empty());
        assert_eq!(config.modes, Default::default());
    }

    #[test]
    fn reports_bad_modes_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".agents");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("modes.toml"),
            "[default]\nclash_handler = \"merge\"",
        )
        .unwrap();
        assert!(matches!(load(tmp.path()), Err(ConfigError::Toml { .. })));
    }
}

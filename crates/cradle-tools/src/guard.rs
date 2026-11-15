//! Confines every path to the project root.
//!
//! - `..`, absolute paths and symlinks that leave the root are refused.
//! - `.git/` is refused entirely (hooks there would run on the user's next commit).
//! - Files matched by `.gitignore` (root file and `.git/info/exclude`) and common secret
//!   files are hidden.
//! - `.agents/` config (`RULES.md`, `modes.toml`, `modes/`) and `agents.toml` are
//!   read-only, except that scaffold mode may write anything under `.agents/`. Other
//!   `.agents/` docs are writable only with user approval.
//! - Files that make other tools run code (build scripts, manifests, CI, editor tasks,
//!   `.gitignore` itself) are writable only with user approval.

use std::io;
use std::path::{Component, Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

/// Always hidden, whether or not `.gitignore` lists them.
const SECRET_PATTERNS: &[&str] = &[
    ".env",
    ".env.*",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "id_rsa*",
    "id_ed25519*",
    "id_ecdsa*",
    ".netrc",
    ".npmrc",
    ".pypirc",
];

/// Writes to these need the user's approval: each one can make a later build, test, git
/// or editor action run code, or expose hidden files.
const PROTECTED_PATTERNS: &[(&str, &str)] = &[
    ("**/build.rs", "build script (runs on every build)"),
    (
        "**/Cargo.toml",
        "manifest (controls dependencies and build scripts)",
    ),
    (".cargo/**", "cargo config (can override build commands)"),
    ("**/Makefile", "build file"),
    ("**/justfile", "build file"),
    ("**/package.json", "manifest (contains runnable scripts)"),
    ("**/*.sh", "shell script"),
    (".github/**", "CI configuration"),
    (".gitlab-ci.yml", "CI configuration"),
    (".vscode/**", "editor tasks"),
    (".idea/**", "editor tasks"),
    (".envrc", "direnv (runs on cd)"),
    ("rust-toolchain", "toolchain override"),
    ("rust-toolchain.toml", "toolchain override"),
    (
        ".gitignore",
        "controls which files are hidden from the agent",
    ),
    (".gitattributes", "can configure git filters that run code"),
    (".gitmodules", "submodule sources"),
    ("providers.toml", "compiled-in provider list"),
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GuardError {
    #[error("`{0}` is outside the project")]
    Outside(String),
    #[error("`.git/` is off limits")]
    GitDir,
    #[error("`{0}` is ignored or secret and hidden from the agent")]
    Hidden(String),
    #[error("`{0}` is project config and read-only for the agent")]
    AgentsConfig(String),
    #[error("invalid path `{0}`")]
    Invalid(String),
}

/// How a write must be handled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteClass {
    Normal,
    /// Needs user approval. The string says why.
    Protected(&'static str),
    /// A `.agents/` doc: needs user approval.
    AgentsDoc,
    /// Project config (`.agents/RULES.md`, `modes.toml`, `modes/`), written in scaffold
    /// mode: needs user approval, and the project must be trusted again.
    AgentsConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub abs: PathBuf,
    /// Relative to the project root, for display and matching.
    pub rel: PathBuf,
}

#[derive(Debug)]
pub struct PathGuard {
    root: PathBuf,
    ignore: Gitignore,
    protected: GlobSet,
    protected_reasons: Vec<&'static str>,
}

impl PathGuard {
    pub fn new(root: &Path) -> io::Result<Self> {
        let root = root.canonicalize()?;

        let mut builder = GitignoreBuilder::new(&root);
        for file in [root.join(".gitignore"), root.join(".git/info/exclude")] {
            if file.is_file() {
                // A malformed line is skipped; the rest of the file still applies.
                let _ = builder.add(file);
            }
        }
        for pattern in SECRET_PATTERNS {
            builder
                .add_line(None, pattern)
                .map_err(|e| io::Error::other(e.to_string()))?;
        }
        let ignore = builder
            .build()
            .map_err(|e| io::Error::other(e.to_string()))?;

        let mut set = GlobSetBuilder::new();
        let mut protected_reasons = Vec::new();
        for (pattern, reason) in PROTECTED_PATTERNS {
            set.add(Glob::new(pattern).map_err(|e| io::Error::other(e.to_string()))?);
            protected_reasons.push(*reason);
        }
        let protected = set.build().map_err(|e| io::Error::other(e.to_string()))?;

        Ok(PathGuard {
            root,
            ignore,
            protected,
            protected_reasons,
        })
    }

    /// The canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a path for reading.
    pub fn resolve(&self, input: &str) -> Result<Resolved, GuardError> {
        let resolved = self.confine(input)?;
        if resolved.rel.components().next() == Some(Component::Normal(".git".as_ref())) {
            return Err(GuardError::GitDir);
        }
        if self.is_hidden(&resolved.rel, resolved.abs.is_dir()) {
            return Err(GuardError::Hidden(display(&resolved.rel)));
        }
        Ok(resolved)
    }

    /// Resolve a path for writing and classify it.
    pub fn resolve_write(&self, input: &str) -> Result<(Resolved, WriteClass), GuardError> {
        let resolved = self.resolve(input)?;
        let rel = &resolved.rel;
        if rel.as_os_str().is_empty() {
            return Err(GuardError::Invalid(input.to_owned()));
        }

        if is_agents_config(rel) {
            return Err(GuardError::AgentsConfig(display(rel)));
        }
        if rel.starts_with(".agents") {
            return Ok((resolved, WriteClass::AgentsDoc));
        }
        if let Some(i) = self.protected.matches(rel).first() {
            let reason = self.protected_reasons[*i];
            return Ok((resolved, WriteClass::Protected(reason)));
        }
        Ok((resolved, WriteClass::Normal))
    }

    /// Resolve a path for writing in scaffold mode: anything under `.agents/` may be
    /// created or changed, config included. `agents.toml` stays read-only; everything else
    /// is classified as by [`PathGuard::resolve_write`].
    pub fn resolve_scaffold(&self, input: &str) -> Result<(Resolved, WriteClass), GuardError> {
        let resolved = self.resolve(input)?;
        if !resolved.rel.starts_with(".agents") {
            return self.resolve_write(input);
        }
        let class = if is_agents_config(&resolved.rel) {
            WriteClass::AgentsConfig
        } else {
            WriteClass::AgentsDoc
        };
        Ok((resolved, class))
    }

    /// Whether `rel` (relative to the root) is hidden by `.gitignore` or secret patterns.
    pub fn is_hidden(&self, rel: &Path, is_dir: bool) -> bool {
        !rel.as_os_str().is_empty()
            && self
                .ignore
                .matched_path_or_any_parents(rel, is_dir)
                .is_ignore()
    }

    /// Lexically normalise, then check the nearest existing ancestor's real path so
    /// symlinks cannot escape.
    fn confine(&self, input: &str) -> Result<Resolved, GuardError> {
        if input.is_empty() || input.contains('\0') {
            return Err(GuardError::Invalid(input.to_owned()));
        }
        let candidate = Path::new(input);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        };

        let mut normal = PathBuf::new();
        for component in joined.components() {
            match component {
                Component::ParentDir => {
                    if !normal.pop() {
                        return Err(GuardError::Outside(input.to_owned()));
                    }
                }
                Component::CurDir => {}
                other => normal.push(other),
            }
        }
        if !normal.starts_with(&self.root) {
            return Err(GuardError::Outside(input.to_owned()));
        }

        // Walk up to the nearest existing ancestor and resolve symlinks there.
        let mut existing = normal.clone();
        let mut rest = Vec::new();
        while !existing.exists() {
            match existing.file_name() {
                Some(name) => rest.push(name.to_owned()),
                None => return Err(GuardError::Outside(input.to_owned())),
            }
            if !existing.pop() {
                return Err(GuardError::Outside(input.to_owned()));
            }
        }
        let mut abs = existing
            .canonicalize()
            .map_err(|_| GuardError::Invalid(input.to_owned()))?;
        if !abs.starts_with(&self.root) {
            return Err(GuardError::Outside(input.to_owned()));
        }
        for name in rest.into_iter().rev() {
            abs.push(name);
        }

        let rel = abs
            .strip_prefix(&self.root)
            .map(Path::to_path_buf)
            .map_err(|_| GuardError::Outside(input.to_owned()))?;
        Ok(Resolved { abs, rel })
    }
}

/// Project config the agent may never change: it decides which commands, modes and
/// preferences the agent gets.
fn is_agents_config(rel: &Path) -> bool {
    rel == Path::new("agents.toml")
        || rel == Path::new(".agents/RULES.md")
        || rel == Path::new(".agents/modes.toml")
        || rel == Path::new(".agents/soul.toml")
        || rel.starts_with(".agents/modes")
}

pub(crate) fn display(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.is_empty() {
        ".".to_owned()
    } else {
        s.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn project() -> (tempfile::TempDir, PathGuard) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join(".git/hooks")).unwrap();
        fs::create_dir_all(root.join(".agents/modes")).unwrap();
        fs::create_dir_all(root.join("target")).unwrap();
        fs::write(root.join(".gitignore"), "target/\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}").unwrap();
        fs::write(root.join(".env"), "SECRET=1").unwrap();
        let guard = PathGuard::new(root).unwrap();
        (tmp, guard)
    }

    #[test]
    fn resolves_inside_paths() {
        let (_tmp, guard) = project();
        let r = guard.resolve("src/main.rs").unwrap();
        assert_eq!(r.rel, Path::new("src/main.rs"));
        let r = guard.resolve("./src/../src/new.rs").unwrap();
        assert_eq!(r.rel, Path::new("src/new.rs"), "missing files resolve too");
        let abs = guard.root().join("src/main.rs");
        assert!(guard.resolve(abs.to_str().unwrap()).is_ok());
    }

    #[test]
    fn refuses_escapes_git_and_hidden() {
        let (_tmp, guard) = project();
        assert!(matches!(
            guard.resolve("../outside.txt"),
            Err(GuardError::Outside(_))
        ));
        assert!(matches!(
            guard.resolve("/etc/passwd"),
            Err(GuardError::Outside(_))
        ));
        assert_eq!(
            guard.resolve(".git/hooks/pre-commit"),
            Err(GuardError::GitDir)
        );
        assert!(matches!(guard.resolve(".env"), Err(GuardError::Hidden(_))));
        assert!(matches!(
            guard.resolve("target/debug/x"),
            Err(GuardError::Hidden(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_escape() {
        let (tmp, guard) = project();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("src/link")).unwrap();
        assert!(matches!(
            guard.resolve("src/link/file"),
            Err(GuardError::Outside(_))
        ));
    }

    #[test]
    fn classifies_writes() {
        let (_tmp, guard) = project();
        assert_eq!(
            guard.resolve_write("src/lib.rs").unwrap().1,
            WriteClass::Normal
        );
        assert!(matches!(
            guard.resolve_write("build.rs").unwrap().1,
            WriteClass::Protected(_)
        ));
        assert!(matches!(
            guard.resolve_write("crates/x/Cargo.toml").unwrap().1,
            WriteClass::Protected(_)
        ));
        assert!(matches!(
            guard.resolve_write(".gitignore").unwrap().1,
            WriteClass::Protected(_)
        ));
        assert_eq!(
            guard.resolve_write(".agents/features.md").unwrap().1,
            WriteClass::AgentsDoc
        );
        assert!(matches!(
            guard.resolve_write(".agents/RULES.md"),
            Err(GuardError::AgentsConfig(_))
        ));
        assert!(
            matches!(
                guard.resolve_write(".agents/soul.toml"),
                Err(GuardError::AgentsConfig(_))
            ),
            "project preferences are config"
        );
        assert_eq!(
            guard.resolve_scaffold(".agents/RULES.md").unwrap().1,
            WriteClass::AgentsConfig,
            "scaffolding may create config"
        );
        fs::write(guard.root().join(".agents/modes.toml"), "").unwrap();
        assert_eq!(
            guard.resolve_scaffold(".agents/modes.toml").unwrap().1,
            WriteClass::AgentsConfig,
            "scaffolding may change existing config"
        );
        assert_eq!(
            guard.resolve_scaffold(".agents/features.md").unwrap().1,
            WriteClass::AgentsDoc
        );
        assert_eq!(
            guard.resolve_scaffold("src/lib.rs").unwrap().1,
            WriteClass::Normal
        );
        assert!(matches!(
            guard.resolve_scaffold("agents.toml"),
            Err(GuardError::AgentsConfig(_))
        ));
        assert!(matches!(
            guard.resolve_write(".agents/modes/plan.md"),
            Err(GuardError::AgentsConfig(_))
        ));
        assert!(matches!(
            guard.resolve_write("agents.toml"),
            Err(GuardError::AgentsConfig(_))
        ));
    }
}

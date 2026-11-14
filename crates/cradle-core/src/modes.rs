//! Resolves the active set of modes from built-in defaults, the user's global modes
//! (`~/.config/cradle/modes/<name>.md`) and the project's modes. See
//! `.agents/roadmap/FEATURE_MODES.md`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use cradle_config::{ClashHandler, Mode, ModeAccess, Modes};

const BUILTIN_PLAN: &str = include_str!("../modes/plan.md");
const BUILTIN_BUILD: &str = include_str!("../modes/build.md");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveMode {
    pub name: String,
    pub access: ModeAccess,
    pub prompt: String,
}

/// How to resolve a mode defined both globally and by the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClashChoice {
    Override,
    Combine,
}

impl ClashChoice {
    /// The choice implied by the project's `clash_handler`, or `None` for `ask`.
    pub fn from_handler(handler: ClashHandler) -> Option<Self> {
        match handler {
            ClashHandler::Override => Some(ClashChoice::Override),
            ClashHandler::Combine => Some(ClashChoice::Combine),
            ClashHandler::Ask => None,
        }
    }
}

/// A mode defined both by the user's global config and the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clash {
    pub mode: String,
    pub global: PathBuf,
    pub project: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeSet {
    modes: Vec<ActiveMode>,
}

impl ModeSet {
    /// `project` is `None` when the project config is not trusted.
    pub fn resolve(
        project: Option<&Modes>,
        global_dir: Option<&Path>,
        choice: ClashChoice,
    ) -> io::Result<Self> {
        let mut modes = Vec::new();
        for (name, builtin, project_mode, access) in [
            (
                "plan",
                BUILTIN_PLAN,
                project.and_then(|m| m.plan.as_ref()),
                ModeAccess::Read,
            ),
            (
                "build",
                BUILTIN_BUILD,
                project.and_then(|m| m.build.as_ref()),
                ModeAccess::Full,
            ),
        ] {
            let global = global_file(global_dir, name);
            let prompt = match (global, project_mode) {
                (None, None) => builtin.to_owned(),
                (Some(g), None) => fs::read_to_string(g)?,
                (None, Some(p)) => fs::read_to_string(&p.path)?,
                (Some(g), Some(p)) => match choice {
                    ClashChoice::Override => fs::read_to_string(&p.path)?,
                    ClashChoice::Combine => format!(
                        "{}\n\nProject-specific instructions follow. Where they contradict the \
                         instructions above, the project instructions win.\n\n{}",
                        fs::read_to_string(g)?.trim_end(),
                        fs::read_to_string(&p.path)?
                    ),
                },
            };
            modes.push(ActiveMode {
                name: name.to_owned(),
                access,
                prompt,
            });
        }
        for Mode { name, path, access } in project.map(|m| m.extra.as_slice()).unwrap_or(&[]) {
            modes.push(ActiveMode {
                name: name.clone(),
                access: *access,
                prompt: fs::read_to_string(path)?,
            });
        }
        Ok(ModeSet { modes })
    }

    /// Default modes the project and the user's global config both define.
    pub fn clashes(project: Option<&Modes>, global_dir: Option<&Path>) -> Vec<Clash> {
        let Some(project) = project else {
            return Vec::new();
        };
        [("plan", &project.plan), ("build", &project.build)]
            .into_iter()
            .filter_map(|(name, mode)| {
                let mode = mode.as_ref()?;
                Some(Clash {
                    mode: name.to_owned(),
                    global: global_file(global_dir, name)?,
                    project: mode.path.clone(),
                })
            })
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&ActiveMode> {
        self.modes.iter().find(|m| m.name == name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.modes.iter().map(|m| m.name.as_str())
    }

    /// The mode after `name`, wrapping around.
    pub fn next_after(&self, name: &str) -> &ActiveMode {
        let i = self.modes.iter().position(|m| m.name == name).unwrap_or(0);
        &self.modes[(i + 1) % self.modes.len()]
    }
}

fn global_file(global_dir: Option<&Path>, name: &str) -> Option<PathBuf> {
    let path = global_dir?.join(format!("{name}.md"));
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_modes(dir: &Path) -> Modes {
        fs::write(dir.join("p_plan.md"), "project plan").unwrap();
        fs::write(dir.join("p_debug.md"), "project debug").unwrap();
        Modes {
            plan: Some(Mode {
                name: "plan".into(),
                path: dir.join("p_plan.md"),
                access: ModeAccess::Read,
            }),
            build: None,
            clash_handler: ClashHandler::Ask,
            extra: vec![Mode {
                name: "debug".into(),
                path: dir.join("p_debug.md"),
                access: ModeAccess::Full,
            }],
        }
    }

    #[test]
    fn builtins_without_project() {
        let set = ModeSet::resolve(None, None, ClashChoice::Override).unwrap();
        assert_eq!(set.names().collect::<Vec<_>>(), ["plan", "build"]);
        assert_eq!(set.get("plan").unwrap().access, ModeAccess::Read);
        assert_eq!(set.get("build").unwrap().access, ModeAccess::Full);
        assert_eq!(set.next_after("build").name, "plan");
    }

    #[test]
    fn project_and_global_clash() {
        let tmp = tempfile::tempdir().unwrap();
        let global = tmp.path().join("global");
        fs::create_dir_all(&global).unwrap();
        fs::write(global.join("plan.md"), "global plan").unwrap();
        let modes = project_modes(tmp.path());

        let clashes = ModeSet::clashes(Some(&modes), Some(&global));
        assert_eq!(clashes.len(), 1);
        assert_eq!(clashes[0].mode, "plan");

        let over = ModeSet::resolve(Some(&modes), Some(&global), ClashChoice::Override).unwrap();
        assert_eq!(over.get("plan").unwrap().prompt, "project plan");
        let comb = ModeSet::resolve(Some(&modes), Some(&global), ClashChoice::Combine).unwrap();
        let prompt = &comb.get("plan").unwrap().prompt;
        assert!(prompt.starts_with("global plan") && prompt.ends_with("project plan"));
        assert_eq!(comb.get("debug").unwrap().access, ModeAccess::Full);
        assert_eq!(
            comb.get("build").unwrap().prompt,
            BUILTIN_BUILD,
            "no clash without a project build mode"
        );
    }
}

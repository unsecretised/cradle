//! Everything that happens before the agent starts: loading and trusting project config,
//! resolving mode clashes, opening the session log, and working out which provider to use.
//!
//! Trust questions are asked on plain stdin before the TUI starts. If no provider is
//! usable (first run, missing key), the TUI shows its setup screen instead.

use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};

use cradle_backends::{Backend, Message};
use cradle_config::soul::Soul;
use cradle_config::user::UserConfig;
use cradle_config::{Detection, ModeAccess, ProjectConfig, SourceKind, paths};
use cradle_core::trust::{self, TrustStore};
use cradle_core::{AgentSetup, ClashChoice, ModeSet, ReviewPolicy, session};
use cradle_provider_spec::ProviderSpec;
use cradle_tools::PathGuard;

use crate::{Error, RunArgs, inspect};

/// What the TUI needs to start.
pub struct Session {
    pub agent: AgentParts,
    pub root: PathBuf,
    pub mode_names: Vec<String>,
    pub resumed: usize,
    pub trusted: bool,
    pub providers: Vec<ProviderSpec>,
    /// A ready backend, or why the setup screen must run (empty on first run).
    pub backend: Result<Backend, String>,
    /// Provider to highlight on the setup screen.
    pub preselect: Option<String>,
    /// Where the setup screen saves the chosen default.
    pub user_config: Option<PathBuf>,
    /// `~/.config/cradle/soul.toml`, where `/preferences` saves.
    pub soul_path: Option<PathBuf>,
    /// The user has no `soul.toml` yet.
    pub soul_missing: bool,
}

/// Everything the agent needs except the backend.
pub struct AgentParts {
    pub guard: PathGuard,
    pub project: Option<ProjectConfig>,
    pub modes: ModeSet,
    pub mode: String,
    pub review: ReviewPolicy,
    pub history: Vec<Message>,
    pub session: Option<session::SessionLog>,
    pub data_dir: Option<PathBuf>,
    pub trust_store: Option<PathBuf>,
    pub global_modes: Option<PathBuf>,
    pub soul: Soul,
}

impl AgentParts {
    pub fn with_backend(self, backend: Backend) -> AgentSetup {
        AgentSetup {
            backend,
            guard: self.guard,
            project: self.project,
            modes: self.modes,
            mode: self.mode,
            review: self.review,
            history: self.history,
            session: self.session,
            data_dir: self.data_dir,
            trust_store: self.trust_store,
            global_modes: self.global_modes,
            soul: self.soul,
        }
    }
}

pub fn prepare(args: &RunArgs) -> Result<Session, Error> {
    let root = args.root.canonicalize().map_err(|source| Error::Io {
        path: args.root.clone(),
        source,
    })?;

    let (providers, _) = inspect::load_providers()?;
    if let Some(id) = &args.provider
        && !providers.iter().any(|p| &p.id == id)
    {
        let ids: Vec<&str> = providers.iter().map(|p| p.id.as_str()).collect();
        return Err(Error::Setup(format!(
            "unknown provider `{id}` (available: {})",
            ids.join(", ")
        )));
    }
    let user_config = paths::user_config();
    let user = match &user_config {
        Some(path) => UserConfig::load(path).map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?,
        None => UserConfig::default(),
    };
    let (backend, preselect) = choose_backend(args, &providers, &user);

    let soul_path = paths::soul();
    let soul = match &soul_path {
        Some(path) => Soul::load(path)?,
        None => None,
    };
    let soul_missing = soul_path.is_some() && soul.is_none();
    let soul = soul.unwrap_or_default();

    let config_dir = paths::config_dir();
    let mut store = match &config_dir {
        Some(dir) => {
            Some(
                TrustStore::load(dir.join("trust.toml")).map_err(|source| Error::Io {
                    path: dir.join("trust.toml"),
                    source,
                })?,
            )
        }
        None => None,
    };

    let project = load_project(&root, store.as_mut())?;
    let trusted = project.is_some();

    let global_modes = config_dir.as_ref().map(|d| d.join("modes"));
    let modes = resolve_modes(
        &root,
        project.as_ref(),
        global_modes.as_deref(),
        store.as_mut(),
    )?;
    if modes.get(&args.mode).is_none() {
        let names: Vec<&str> = modes.names().collect();
        return Err(Error::Setup(format!(
            "unknown mode `{}` (available: {})",
            args.mode,
            names.join(", ")
        )));
    }

    let data_dir = paths::data_dir();
    let (history, log) = match &data_dir {
        Some(dir) if args.resume => match session::latest(dir, &root) {
            Ok(Some((path, messages))) => (messages, session::SessionLog::open(path).ok()),
            Ok(None) => {
                println!("No earlier conversation in this project; starting a new one.");
                (Vec::new(), session::SessionLog::create(dir, &root).ok())
            }
            Err(e) => return Err(Error::Setup(format!("cannot resume: {e}"))),
        },
        Some(dir) => (Vec::new(), session::SessionLog::create(dir, &root).ok()),
        None => (Vec::new(), None),
    };

    let guard = PathGuard::new(&root).map_err(|source| Error::Io {
        path: root.clone(),
        source,
    })?;
    let mode_names = modes.names().map(str::to_owned).collect();
    Ok(Session {
        resumed: history.len(),
        root,
        mode_names,
        trusted,
        providers,
        backend,
        preselect,
        user_config,
        soul_path,
        soul_missing,
        agent: AgentParts {
            guard,
            project,
            modes,
            mode: args.mode.clone(),
            review: args.review.into(),
            history,
            session: log,
            data_dir,
            trust_store: config_dir.as_ref().map(|d| d.join("trust.toml")),
            global_modes,
            soul,
        },
    })
}

/// Use `--provider`, else the saved default. Returns the backend, or why setup is needed,
/// and which provider to highlight on the setup screen.
fn choose_backend(
    args: &RunArgs,
    providers: &[ProviderSpec],
    user: &UserConfig,
) -> (Result<Backend, String>, Option<String>) {
    let saved = &user.default;
    let Some(id) = args.provider.clone().or_else(|| saved.provider.clone()) else {
        return (Err(String::new()), None);
    };
    if args.setup {
        return (Err(String::new()), Some(id));
    }
    let Some(spec) = providers.iter().find(|p| p.id == id) else {
        return (
            Err(format!("Your saved provider `{id}` no longer exists.")),
            None,
        );
    };
    // The saved model applies only to the saved provider.
    let model = args.model.clone().or_else(|| {
        (saved.provider.as_deref() == Some(id.as_str()))
            .then(|| saved.model.clone())
            .flatten()
    });
    match Backend::new(spec, model.as_deref()) {
        Ok(backend) => (Ok(backend), Some(id)),
        Err(e) => (Err(format!("Cannot use `{id}`: {e}.")), Some(id)),
    }
}

/// Load the project config, asking which form to use if both exist and whether to trust
/// it if it is new or changed. Returns `None` if there is none or it is not trusted.
fn load_project(
    root: &Path,
    mut store: Option<&mut TrustStore>,
) -> Result<Option<ProjectConfig>, Error> {
    let source = match cradle_config::detect(root)? {
        Detection::None => return Ok(None),
        Detection::One(source) => source,
        Detection::Both => match store.as_deref().and_then(|s| s.source_choice(root)) {
            Some(source) => source,
            None => {
                println!("This project has both .agents/ and agents.toml.");
                let answer = ask("Use [d]irectory or [f]ile?", &["d", "f"])?;
                let source = if answer == "d" {
                    SourceKind::Directory
                } else {
                    SourceKind::File
                };
                if let Some(store) = store.as_deref_mut() {
                    let _ = store.set_source_choice(root, source);
                }
                source
            }
        },
    };
    let config = cradle_config::load(root, source)?;
    let snapshot = trust::snapshot(&config).map_err(|source| Error::Io {
        path: root.to_owned(),
        source,
    })?;
    let hash = trust::snapshot_hash(&snapshot);

    if store.as_deref().is_some_and(|s| s.is_trusted(root, &hash)) {
        return Ok(Some(config));
    }

    print_trust_summary(&config);
    let approved = store.as_deref().and_then(|s| s.approved_snapshot(root));
    if approved.is_some() {
        println!("What changed since you last trusted it:\n");
    } else {
        println!("The full config:\n");
    }
    println!("{}", trust::config_diff(approved.as_deref(), &snapshot));
    if ask("Trust this project config? [y/N]", &["y", "n", ""])? != "y" {
        println!("Not trusted: running without project modes and commands.\n");
        return Ok(None);
    }
    if let Some(store) = store {
        store.trust(root, &snapshot).map_err(|source| Error::Io {
            path: root.to_owned(),
            source,
        })?;
    }
    Ok(Some(config))
}

fn print_trust_summary(config: &ProjectConfig) {
    let source = match config.source {
        SourceKind::Directory => ".agents/",
        SourceKind::File => "agents.toml",
    };
    println!("\nThis project's config ({source}) is new or has changed since you last trusted it.");
    println!("If you trust it, the agent will be allowed to request these commands:");
    if config.commands.is_empty() {
        println!("  (none)");
    }
    for (name, line) in &config.commands {
        println!("  {name:<12} {line}");
    }
    println!("and these modes will apply:");
    let m = &config.modes;
    for (name, mode) in [("plan", &m.plan), ("build", &m.build)] {
        if let Some(mode) = mode {
            println!("  {name:<12} {}", mode.path.display());
        }
    }
    for mode in &m.extra {
        let access = match mode.access {
            ModeAccess::Read => "read-only",
            ModeAccess::Full => "full access",
        };
        println!("  {:<12} {} ({access})", mode.name, mode.path.display());
    }
    if !m.commands.is_empty() {
        println!("and these mode commands may run without asking, once a reviewer passes them:");
        for (mode, commands) in &m.commands {
            for (name, line) in commands {
                println!("  {mode:<12} {name}: {line}");
            }
        }
    }
    if config.docs.rules.is_some() {
        println!("Project rules (RULES.md) will be added to the prompt.");
    }
    println!();
}

fn resolve_modes(
    root: &Path,
    project: Option<&ProjectConfig>,
    global_dir: Option<&Path>,
    store: Option<&mut TrustStore>,
) -> Result<ModeSet, Error> {
    let project_modes = project.map(|p| &p.modes);
    let clashes = ModeSet::clashes(project_modes, global_dir);

    let choice = match project_modes.and_then(|m| ClashChoice::from_handler(m.clash_handler)) {
        Some(choice) => choice,
        None if clashes.is_empty() => ClashChoice::Override,
        None => match store.as_deref().and_then(|s| s.clash_choice(root)) {
            Some(choice) => choice,
            None => {
                println!("Both your global config and this project define these modes:");
                for clash in &clashes {
                    println!(
                        "  {}: {} and {}",
                        clash.mode,
                        clash.global.display(),
                        clash.project.display()
                    );
                }
                let answer = ask(
                    "Use the project's version ([o]verride) or both ([c]ombine)?",
                    &["c", "o"],
                )?;
                let choice = if answer == "c" {
                    ClashChoice::Combine
                } else {
                    ClashChoice::Override
                };
                if let Some(store) = store {
                    let _ = store.set_clash_choice(root, choice);
                }
                choice
            }
        },
    };

    ModeSet::resolve(project_modes, global_dir, choice)
        .map_err(|e| Error::Setup(format!("cannot read mode files: {e}")))
}

/// Ask on stdin until the answer is one of `allowed` (lowercased). Without a terminal,
/// the answer is the last allowed value (the safe default).
fn ask(prompt: &str, allowed: &[&str]) -> Result<String, Error> {
    let fallback = allowed.last().copied().unwrap_or("").to_owned();
    if !io::stdin().is_terminal() {
        println!("{prompt} (no terminal: answering `{fallback}`)");
        return Ok(fallback);
    }
    let stdin = io::stdin();
    loop {
        print!("{prompt} ");
        io::stdout().flush().map_err(Error::Terminal)?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).map_err(Error::Terminal)? == 0 {
            return Ok(fallback);
        }
        let answer = line.trim().to_ascii_lowercase();
        if allowed.contains(&answer.as_str()) {
            return Ok(answer);
        }
    }
}

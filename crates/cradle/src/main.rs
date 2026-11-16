mod inspect;
mod setup;
mod tui;

use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use cradle_backends::BackendError;
use cradle_config::{ConfigError, SourceKind};
use cradle_core::ReviewPolicy;

#[derive(Parser)]
#[command(name = "cradle", version, about = "A coding-agent harness")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    #[command(flatten)]
    run: RunArgs,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Project root.
    #[arg(long, default_value = ".")]
    root: PathBuf,
    /// Provider id from `cradle providers` (default: the one chosen during setup).
    #[arg(long)]
    provider: Option<String>,
    /// Model (defaults to the provider's default model).
    #[arg(long)]
    model: Option<String>,
    /// Mode to start in.
    #[arg(long, default_value = "plan")]
    mode: String,
    /// Who approves commands: you, or a reviewer subagent (falls back to you).
    #[arg(long, value_enum, default_value = "user")]
    review: ReviewArg,
    /// Show the provider setup screen even if a default is configured.
    #[arg(long)]
    setup: bool,
    /// Continue the most recent conversation in this project.
    #[arg(long = "continue", short = 'c')]
    resume: bool,
    /// Continue a specific conversation: its id from /session, or a unique prefix of it.
    #[arg(long = "resume", value_name = "ID", conflicts_with = "resume")]
    resume_id: Option<String>,
}

#[derive(Subcommand)]
enum Command {
    /// List providers: compiled-in, plus ~/.config/cradle/providers.toml.
    Providers,
    /// Show the project config Cradle would load.
    Config {
        /// Project root.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Which form to load when both .agents/ and agents.toml exist.
        #[arg(long, value_enum)]
        source: Option<SourceArg>,
    },
    /// Check ~/.config/cradle/theme.toml, or print a commented example.
    Theme {
        /// Print the built-in dark theme, with every setting explained.
        #[arg(long)]
        example: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SourceArg {
    /// The .agents/ directory.
    Dir,
    /// The agents.toml file.
    File,
}

#[derive(Clone, Copy, ValueEnum)]
enum ReviewArg {
    User,
    Subagent,
}

impl From<ReviewArg> for ReviewPolicy {
    fn from(arg: ReviewArg) -> Self {
        match arg {
            ReviewArg::User => ReviewPolicy::User,
            ReviewArg::Subagent => ReviewPolicy::Subagent,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("invalid {}:\n{errors}", path.display())]
    Providers {
        path: PathBuf,
        errors: cradle_provider_spec::Errors,
    },

    #[error("cannot read {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },

    #[error(transparent)]
    Backend(#[from] BackendError),

    #[error("{0}")]
    Setup(String),

    #[error("terminal error: {0}")]
    Terminal(#[source] io::Error),
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Some(Command::Providers) => inspect::list_providers(),
        Some(Command::Config { root, source }) => config(&root, source),
        Some(Command::Theme { example }) => {
            theme(example);
            Ok(())
        }
        None => run(cli.run).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            if matches!(e, Error::Config(ConfigError::Clash(_))) {
                eprintln!("hint: pass --source dir or --source file");
            }
            ExitCode::FAILURE
        }
    }
}

fn config(root: &std::path::Path, source: Option<SourceArg>) -> Result<(), Error> {
    let config = match source {
        None => cradle_config::load_detected(root)?,
        Some(SourceArg::Dir) => cradle_config::load(root, SourceKind::Directory)?,
        Some(SourceArg::File) => cradle_config::load(root, SourceKind::File)?,
    };
    inspect::print_config(&config);
    Ok(())
}

/// `cradle theme`: where the theme comes from and any problems in it. Problems are not
/// errors: the TUI starts anyway and fills in from the preset.
fn theme(example: bool) {
    if example {
        print!("{}", tui::theme::DARK);
        return;
    }
    let path = cradle_config::paths::theme();
    let (theme, warnings) = tui::theme::load(path.as_deref());
    match &path {
        None => println!("No config directory; using the {} preset.", theme.preset),
        Some(p) if p.is_file() => {
            println!("{} (on the {} preset)", p.display(), theme.preset)
        }
        Some(p) => {
            println!("No {}; using the {} preset.", p.display(), theme.preset);
            println!("Start one with: cradle theme --example > {}", p.display());
        }
    }
    if warnings.is_empty() {
        println!("No problems.");
    } else {
        println!("{} problem(s), skipped:", warnings.len());
        for w in warnings {
            println!("  {w}");
        }
    }
}

async fn run(args: RunArgs) -> Result<(), Error> {
    let session = setup::prepare(&args)?;
    tui::run(session).await
}

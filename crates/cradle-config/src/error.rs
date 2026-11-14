use std::io;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no project config in {} (expected .agents/ or agents.toml)", .0.display())]
    NotFound(PathBuf),

    #[error("both .agents/ and agents.toml exist in {}; choose one", .0.display())]
    Clash(PathBuf),

    #[error("cannot read {}: {source}", path.display())]
    Io { path: PathBuf, source: io::Error },

    #[error("invalid TOML in {}: {source}", path.display())]
    Toml {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("mode `{name}` (`{path}`): {reason}")]
    ModePath {
        name: String,
        path: String,
        reason: String,
    },

    #[error("{}: {message}", path.display())]
    Invalid { path: PathBuf, message: String },
}

impl ConfigError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        ConfigError::Io {
            path: path.into(),
            source,
        }
    }
}

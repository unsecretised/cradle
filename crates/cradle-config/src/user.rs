//! User preferences in `~/.config/cradle/config.toml`. No secrets live here; API keys
//! go in the OS keychain.
//!
//! ```toml
//! [default]
//! provider = "anthropic"
//! model = "claude-opus-5-5"
//! ```

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfig {
    #[serde(default)]
    pub default: Defaults,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl UserConfig {
    /// Load, treating a missing file as empty.
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(src) => toml::from_str(&src).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: {e}", path.display()),
                )
            }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string(self).map_err(io::Error::other)?;
        fs::write(path, text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cfg/config.toml");
        assert_eq!(UserConfig::load(&path).unwrap(), UserConfig::default());

        let config = UserConfig {
            default: Defaults {
                provider: Some("anthropic".into()),
                model: Some("claude-opus-5-5".into()),
            },
        };
        config.save(&path).unwrap();
        assert_eq!(UserConfig::load(&path).unwrap(), config);
    }
}

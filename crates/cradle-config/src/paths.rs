//! User-level locations. Cradle uses XDG-style paths on every platform so the docs can say
//! `~/.config/cradle` everywhere.

use std::env;
use std::path::PathBuf;

/// `$CRADLE_CONFIG_DIR`, else `$XDG_CONFIG_HOME/cradle`, else `~/.config/cradle`.
pub fn config_dir() -> Option<PathBuf> {
    dir("CRADLE_CONFIG_DIR", "XDG_CONFIG_HOME", ".config")
}

/// `$CRADLE_DATA_DIR`, else `$XDG_DATA_HOME/cradle`, else `~/.local/share/cradle`.
pub fn data_dir() -> Option<PathBuf> {
    dir("CRADLE_DATA_DIR", "XDG_DATA_HOME", ".local/share")
}

/// User preferences: `<config_dir>/config.toml`.
pub fn user_config() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.toml"))
}

/// The TUI theme: `<config_dir>/theme.toml`.
pub fn theme() -> Option<PathBuf> {
    config_dir().map(|d| d.join("theme.toml"))
}

/// The runtime providers overlay: `<config_dir>/providers.toml`.
pub fn providers_overlay() -> Option<PathBuf> {
    config_dir().map(|d| d.join("providers.toml"))
}

fn dir(override_var: &str, xdg_var: &str, home_rel: &str) -> Option<PathBuf> {
    if let Some(p) = non_empty(override_var) {
        return Some(p);
    }
    if let Some(p) = non_empty(xdg_var) {
        return Some(p.join("cradle"));
    }
    non_empty("HOME")
        .or_else(|| non_empty("USERPROFILE"))
        .map(|home| home.join(home_rel).join("cradle"))
}

fn non_empty(var: &str) -> Option<PathBuf> {
    env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

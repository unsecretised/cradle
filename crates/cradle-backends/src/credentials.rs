//! API keys: an env var wins, then the OS keychain (service `cradle`, account = the env
//! var name, e.g. `ANTHROPIC_API_KEY`). Keys are never written to files.
//!
//! Set `CRADLE_NO_KEYCHAIN=1` to skip the keychain (CI, tests).

const SERVICE: &str = "cradle";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env,
    Keychain,
}

/// Where the key named `env` is available, if anywhere.
pub fn find_key(env: &str) -> Option<(String, KeySource)> {
    if let Ok(key) = std::env::var(env)
        && !key.trim().is_empty()
    {
        return Some((key, KeySource::Env));
    }
    if keychain_disabled() {
        return None;
    }
    keyring::Entry::new(SERVICE, env)
        .and_then(|entry| entry.get_password())
        .ok()
        .filter(|key| !key.trim().is_empty())
        .map(|key| (key, KeySource::Keychain))
}

/// Store a key in the OS keychain under `env`.
pub fn save_key(env: &str, key: &str) -> Result<(), String> {
    if keychain_disabled() {
        return Err("the keychain is disabled (CRADLE_NO_KEYCHAIN is set)".into());
    }
    keyring::Entry::new(SERVICE, env)
        .and_then(|entry| entry.set_password(key))
        .map_err(|e| format!("could not save to the OS keychain: {e}"))
}

/// A human name for the platform's credential store.
pub fn store_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS Keychain"
    } else if cfg!(windows) {
        "Windows Credential Manager"
    } else {
        "system keyring (Secret Service)"
    }
}

fn keychain_disabled() -> bool {
    std::env::var_os("CRADLE_NO_KEYCHAIN").is_some_and(|v| !v.is_empty() && v != "0")
}

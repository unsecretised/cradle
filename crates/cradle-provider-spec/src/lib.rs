//! Provider declarations for Cradle.
//!
//! The same schema is used for the compile-time `providers.toml` at the workspace root
//! (validated by `cradle-backends/build.rs`) and the runtime overlay in
//! `~/.config/cradle/providers.toml`. See `.agents/roadmap/FEATURE_PROVIDERS_TOML.md`.

mod codegen;
mod error;
mod raw;
mod validate;

use std::fmt;

use serde::Deserialize;

pub use codegen::to_rust_source;
pub use error::{Error, Errors};
pub use raw::{AuthMode, ProvidersFile, RawCapabilities, RawProvider};

/// A validated provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSpec {
    pub id: String,
    pub backend: Backend,
}

impl ProviderSpec {
    pub fn kind(&self) -> Kind {
        match &self.backend {
            Backend::Http(http) => match http.api {
                HttpApi::Anthropic => Kind::Anthropic,
                HttpApi::OpenaiCompatible => Kind::OpenaiCompatible,
            },
            Backend::Cli(cli) => match cli.agent {
                CliAgent::ClaudeCode => Kind::ClaudeCode,
                CliAgent::Codex => Kind::Codex,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Anthropic,
    OpenaiCompatible,
    ClaudeCode,
    Codex,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Anthropic => "anthropic",
            Kind::OpenaiCompatible => "openai_compatible",
            Kind::ClaudeCode => "claude_code",
            Kind::Codex => "codex",
        }
    }

    pub fn is_cli(self) -> bool {
        matches!(self, Kind::ClaudeCode | Kind::Codex)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    /// Cradle runs the agent loop and calls the HTTP API (a `ModelBackend`).
    Http(HttpProvider),
    /// An external CLI runs the agent loop with Cradle's tools (an `AgentBackend`).
    Cli(CliProvider),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpProvider {
    pub api: HttpApi,
    pub base_url: String,
    pub auth: Auth,
    pub models: Vec<String>,
    pub default_model: Option<String>,
    /// Extra request headers. Credentials are rejected during validation.
    pub headers: Vec<(String, String)>,
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpApi {
    Anthropic,
    OpenaiCompatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Auth {
    None,
    /// The key is looked up in this env var first, then in the OS keychain under the same name.
    ApiKey {
        env: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capabilities {
    pub tools: bool,
    pub streaming: bool,
    pub prompt_caching: bool,
    pub reasoning: bool,
}

impl Capabilities {
    pub fn defaults_for(api: HttpApi) -> Self {
        match api {
            HttpApi::Anthropic => Capabilities {
                tools: true,
                streaming: true,
                prompt_caching: true,
                reasoning: true,
            },
            HttpApi::OpenaiCompatible => Capabilities {
                tools: true,
                streaming: true,
                prompt_caching: false,
                reasoning: false,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliProvider {
    pub agent: CliAgent,
    /// Binary name (looked up on PATH) or absolute path.
    pub binary: String,
    /// Supported version range, as a semver requirement (e.g. `">=2.0, <3.0"`).
    pub version: Option<String>,
    pub models: Vec<String>,
    pub default_model: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliAgent {
    ClaudeCode,
    Codex,
}

impl CliAgent {
    pub fn default_binary(self) -> &'static str {
        match self {
            CliAgent::ClaudeCode => "claude",
            CliAgent::Codex => "codex",
        }
    }
}

/// Parse a providers file without validating it.
pub fn parse(src: &str) -> Result<ProvidersFile, Errors> {
    toml::from_str(src).map_err(|e| Errors::single(None, e.to_string()))
}

/// Parse and validate a complete providers file (the compile-time `providers.toml`).
pub fn parse_and_validate(src: &str) -> Result<Vec<ProviderSpec>, Errors> {
    validate::validate(&parse(src)?.providers)
}

/// Apply a runtime overlay on top of already-validated providers.
///
/// Entries are matched by `id`. A new `id` adds a provider. An existing `id` overrides
/// that provider field by field, unless the overlay sets a different `kind`, in which case
/// it replaces the provider entirely.
pub fn merge(base: &[ProviderSpec], overlay: ProvidersFile) -> Result<Vec<ProviderSpec>, Errors> {
    validate::check_duplicate_ids(&overlay.providers)?;

    let mut merged: Vec<RawProvider> = base.iter().map(RawProvider::from).collect();
    for top in overlay.providers {
        match merged.iter_mut().find(|p| p.id == top.id) {
            Some(existing) => *existing = std::mem::take(existing).overlay(top),
            None => merged.push(top),
        }
    }
    validate::validate(&merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = r#"
        [[provider]]
        id = "anthropic"
        kind = "anthropic"
        base_url = "https://api.anthropic.com"
        api_key_env = "ANTHROPIC_API_KEY"
        models = ["a", "b"]
        default_model = "a"

        [[provider]]
        id = "claude-code"
        kind = "claude_code"
    "#;

    fn base() -> Vec<ProviderSpec> {
        parse_and_validate(BASE).unwrap()
    }

    #[test]
    fn overlay_adds_new_provider() {
        let merged = merge(
            &base(),
            parse(
                "[[provider]]\nid = \"ollama\"\nkind = \"openai_compatible\"\nbase_url = \"http://localhost:11434/v1\"\nauth = \"none\"",
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[2].id, "ollama");
    }

    #[test]
    fn overlay_overrides_fields_by_id() {
        let merged = merge(
            &base(),
            parse("[[provider]]\nid = \"anthropic\"\ndefault_model = \"b\"\n[provider.capabilities]\nreasoning = false").unwrap(),
        )
        .unwrap();
        let Backend::Http(http) = &merged[0].backend else {
            panic!()
        };
        assert_eq!(http.default_model.as_deref(), Some("b"));
        assert_eq!(
            http.base_url, "https://api.anthropic.com",
            "untouched fields are kept"
        );
        assert!(!http.capabilities.reasoning);
        assert!(
            http.capabilities.prompt_caching,
            "other capabilities are kept"
        );
    }

    #[test]
    fn overlay_with_new_kind_replaces_entry() {
        let merged = merge(
            &base(),
            parse("[[provider]]\nid = \"anthropic\"\nkind = \"claude_code\"").unwrap(),
        )
        .unwrap();
        assert_eq!(merged[0].kind(), Kind::ClaudeCode);
    }

    #[test]
    fn overlay_is_validated() {
        let err = merge(
            &base(),
            parse("[[provider]]\nid = \"anthropic\"\ndefault_model = \"zzz\"").unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("is not in `models`"), "{err}");

        let err = merge(
            &base(),
            parse("[[provider]]\nid = \"new\"\nkind = \"codex\"\n[[provider]]\nid = \"new\"\nkind = \"codex\"").unwrap(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("declared more than once"), "{err}");
    }

    #[test]
    fn spec_round_trips_through_raw() {
        for spec in base() {
            let again = validate::validate(&[RawProvider::from(&spec)]).unwrap();
            assert_eq!(again, vec![spec]);
        }
    }
}

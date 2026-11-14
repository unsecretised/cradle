use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{Auth, Backend, Kind, ProviderSpec};

/// The on-disk shape of a providers file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvidersFile {
    #[serde(default, rename = "provider")]
    pub providers: Vec<RawProvider>,
}

/// One `[[provider]]` entry before validation. Every field except `id` is optional so
/// that runtime overlays can override individual fields.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawProvider {
    pub id: String,
    pub kind: Option<Kind>,
    pub base_url: Option<String>,
    pub api_key_env: Option<String>,
    pub auth: Option<AuthMode>,
    pub models: Option<Vec<String>>,
    pub default_model: Option<String>,
    pub headers: Option<BTreeMap<String, String>>,
    pub capabilities: Option<RawCapabilities>,
    pub binary: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMode {
    Key,
    None,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCapabilities {
    pub tools: Option<bool>,
    pub streaming: Option<bool>,
    pub prompt_caching: Option<bool>,
    pub reasoning: Option<bool>,
}

impl RawProvider {
    /// Lay `top` over `self`. A different explicit `kind` replaces the entry entirely,
    /// because the fields of one kind are mostly invalid for another.
    pub(crate) fn overlay(self, top: RawProvider) -> RawProvider {
        if top.kind.is_some() && top.kind != self.kind {
            return top;
        }
        RawProvider {
            id: top.id,
            kind: self.kind,
            base_url: top.base_url.or(self.base_url),
            api_key_env: top.api_key_env.or(self.api_key_env),
            auth: top.auth.or(self.auth),
            models: top.models.or(self.models),
            default_model: top.default_model.or(self.default_model),
            headers: top.headers.or(self.headers),
            capabilities: match (self.capabilities, top.capabilities) {
                (Some(base), Some(top)) => Some(RawCapabilities {
                    tools: top.tools.or(base.tools),
                    streaming: top.streaming.or(base.streaming),
                    prompt_caching: top.prompt_caching.or(base.prompt_caching),
                    reasoning: top.reasoning.or(base.reasoning),
                }),
                (base, top) => top.or(base),
            },
            binary: top.binary.or(self.binary),
            version: top.version.or(self.version),
        }
    }
}

impl From<&ProviderSpec> for RawProvider {
    fn from(spec: &ProviderSpec) -> Self {
        let mut raw = RawProvider {
            id: spec.id.clone(),
            kind: Some(spec.kind()),
            ..Default::default()
        };
        match &spec.backend {
            Backend::Http(http) => {
                raw.base_url = Some(http.base_url.clone());
                match &http.auth {
                    Auth::None => raw.auth = Some(AuthMode::None),
                    Auth::ApiKey { env } => {
                        raw.auth = Some(AuthMode::Key);
                        raw.api_key_env = Some(env.clone());
                    }
                }
                raw.models = Some(http.models.clone());
                raw.default_model = http.default_model.clone();
                raw.headers = Some(http.headers.iter().cloned().collect());
                raw.capabilities = Some(RawCapabilities {
                    tools: Some(http.capabilities.tools),
                    streaming: Some(http.capabilities.streaming),
                    prompt_caching: Some(http.capabilities.prompt_caching),
                    reasoning: Some(http.capabilities.reasoning),
                });
            }
            Backend::Cli(cli) => {
                raw.binary = Some(cli.binary.clone());
                raw.version = cli.version.clone();
                raw.models = Some(cli.models.clone());
                raw.default_model = cli.default_model.clone();
            }
        }
        raw
    }
}

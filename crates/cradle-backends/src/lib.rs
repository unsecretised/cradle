//! Model backends (Anthropic, OpenAI-compatible), where Cradle runs the agent loop, and
//! agent backends (Claude Code, Codex), where an installed CLI runs it with Cradle's tools.
//! See `.agents/roadmap/FEATURE_API_BACKENDS.md` and `.agents/roadmap/FEATURE_CLI_BACKENDS.md`.

mod anthropic;
pub mod cli;
pub mod credentials;
mod error;
mod http;
pub mod message;
mod models;
mod openai;

use cradle_provider_spec::{Auth, Backend as SpecBackend, CliAgent, Errors, HttpApi, ProviderSpec};

pub use cli::{AgentOutcome, AgentTurn, McpEndpoint};
pub use error::BackendError;
pub use message::{
    Block, Message, ModelRequest, Role, StopReason, StreamEvent, ToolSpec, Turn, Usage,
};
pub use models::available_models;

/// A configured connection to one provider and model.
pub struct Backend {
    provider: String,
    /// `None` for a CLI using its own default model.
    model: Option<String>,
    tools: bool,
    inner: Inner,
}

/// Shows the provider and model only; never the API key.
impl std::fmt::Debug for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Backend")
            .field("provider", &self.provider)
            .field("model", &self.model())
            .finish_non_exhaustive()
    }
}

enum Inner {
    Anthropic(anthropic::Client),
    OpenAi(openai::Client),
    ClaudeCode(cli::claude::Client),
    Codex(cli::codex::Client),
}

impl Backend {
    /// Connect to `spec`, using `model` or the provider's default.
    ///
    /// For CLI providers this checks that the CLI is installed and in the supported
    /// version range.
    pub fn new(spec: &ProviderSpec, model: Option<&str>) -> Result<Self, BackendError> {
        let http = match &spec.backend {
            SpecBackend::Http(http) => http,
            SpecBackend::Cli(c) => {
                cli::check_version(&spec.id, &c.binary, c.version.as_deref())?;
                let model = model.map(str::to_owned).or_else(|| c.default_model.clone());
                let inner = match c.agent {
                    CliAgent::ClaudeCode => {
                        Inner::ClaudeCode(cli::claude::Client::new(c.binary.clone(), model.clone()))
                    }
                    CliAgent::Codex => {
                        Inner::Codex(cli::codex::Client::new(c.binary.clone(), model.clone()))
                    }
                };
                return Ok(Backend {
                    provider: spec.id.clone(),
                    model,
                    tools: true,
                    inner,
                });
            }
        };
        let model = model
            .map(str::to_owned)
            .or_else(|| http.default_model.clone())
            .or_else(|| http.models.first().cloned())
            .ok_or_else(|| BackendError::NoModel(spec.id.clone()))?;
        let key = match &http.auth {
            Auth::None => None,
            Auth::ApiKey { env } => Some(
                credentials::find_key(env)
                    .map(|(key, _)| key)
                    .ok_or_else(|| BackendError::MissingKey(env.clone()))?,
            ),
        };
        let inner = match http.api {
            HttpApi::Anthropic => Inner::Anthropic(anthropic::Client::new(
                &http.base_url,
                key,
                model.clone(),
                http.capabilities,
            )?),
            HttpApi::OpenaiCompatible => Inner::OpenAi(openai::Client::new(
                &http.base_url,
                key,
                model.clone(),
                http.headers.clone(),
                http.capabilities,
            )?),
        };
        Ok(Backend {
            provider: spec.id.clone(),
            model: Some(model),
            tools: http.capabilities.tools,
            inner,
        })
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    /// The model name, or `default` for a CLI using its own default.
    pub fn model(&self) -> &str {
        self.model.as_deref().unwrap_or("default")
    }

    /// Whether the provider supports tool calls. Without them the agent can only chat.
    pub fn supports_tools(&self) -> bool {
        self.tools
    }

    /// Whether an installed CLI runs the agent loop (use [`Backend::run_agent_turn`]).
    pub fn is_cli(&self) -> bool {
        matches!(self.inner, Inner::ClaudeCode(_) | Inner::Codex(_))
    }

    /// Send one request. `on_event` receives text and thinking as they arrive.
    ///
    /// CLI backends support this only without tools (one-off requests such as reviews):
    /// the user messages are sent as a single prompt in a fresh, unsaved session.
    pub async fn complete(
        &self,
        req: &ModelRequest<'_>,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<Turn, BackendError> {
        let one_off = |client_outcome: AgentOutcome| Turn {
            message: Message {
                role: Role::Assistant,
                content: vec![Block::Text {
                    text: client_outcome.text,
                }],
            },
            stop: StopReason::EndTurn,
            usage: client_outcome.usage,
        };
        match &self.inner {
            Inner::Anthropic(client) => client.complete(req, on_event).await,
            Inner::OpenAi(client) => client.complete(req, on_event).await,
            Inner::ClaudeCode(_) | Inner::Codex(_) if !req.tools.is_empty() => Err(
                BackendError::Cli("CLI backends take tools through MCP, not requests".into()),
            ),
            Inner::ClaudeCode(client) => {
                let prompt = user_text(req.messages);
                Ok(one_off(
                    client.complete(req.system, &prompt, on_event).await?,
                ))
            }
            Inner::Codex(client) => {
                let prompt = user_text(req.messages);
                Ok(one_off(
                    client.complete(req.system, &prompt, on_event).await?,
                ))
            }
        }
    }

    /// Run one user turn through a CLI backend. Tool calls arrive on Cradle's MCP server
    /// while this future runs; dropping it kills the CLI.
    pub async fn run_agent_turn(
        &self,
        turn: &AgentTurn,
        on_event: &mut (dyn FnMut(StreamEvent) + Send),
    ) -> Result<AgentOutcome, BackendError> {
        match &self.inner {
            Inner::ClaudeCode(client) => client.turn(turn, on_event).await,
            Inner::Codex(client) => client.turn(turn, on_event).await,
            _ => Err(BackendError::Cli(format!(
                "`{}` is not a CLI backend",
                self.provider
            ))),
        }
    }
}

fn user_text(messages: &[Message]) -> String {
    messages
        .iter()
        .filter(|m| m.role == Role::User)
        .map(Message::text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Includes the provider table that `build.rs` generated from `providers.toml`.
///
/// It expands to `include!` of this crate's `OUT_DIR`, so it only works inside
/// `cradle-backends`. Other crates use [`builtin_providers`].
macro_rules! providers {
    () => {
        include!(concat!(env!("OUT_DIR"), "/providers.rs"));
    };
}

mod generated {
    providers!();
}

/// Providers compiled in from the workspace `providers.toml`.
pub use generated::builtin_providers;

/// The compiled-in providers with an optional runtime overlay (the contents of
/// `~/.config/cradle/providers.toml`) applied on top.
pub fn providers(overlay: Option<&str>) -> Result<Vec<ProviderSpec>, Errors> {
    let builtin = builtin_providers();
    match overlay {
        None => Ok(builtin),
        Some(src) => cradle_provider_spec::merge(&builtin, cradle_provider_spec::parse(src)?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_table_matches_source_file() {
        let expected =
            cradle_provider_spec::parse_and_validate(include_str!(env!("CRADLE_PROVIDERS_SOURCE")))
                .unwrap();
        assert_eq!(builtin_providers(), expected);
    }

    #[test]
    fn overlay_is_applied() {
        let merged = providers(Some(
            "[[provider]]\nid = \"cradle-test-extra\"\nkind = \"codex\"",
        ))
        .unwrap();
        assert_eq!(merged.len(), builtin_providers().len() + 1);
        assert_eq!(merged.last().unwrap().id, "cradle-test-extra");
    }
}

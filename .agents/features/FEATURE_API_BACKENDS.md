# API backends

For HTTP providers, Cradle runs the agent loop itself (`cradle-backends`).

## Adapters

- **anthropic** (`anthropic.rs`): Messages API with SSE streaming, tool use, prompt
  caching and thinking blocks. Its message format (`message.rs`) is Cradle's canonical
  internal format, and history is append-only as thinking blocks require.
- **openai_compatible** (`openai.rs`): Chat Completions, converted down from the
  canonical format. Covers OpenAI, Ollama and other compatible servers. Extra `headers`
  from `providers.toml` are sent.

## Behaviour

- Capabilities from `providers.toml` decide whether tools, streaming, caching and reasoning
  parameters are sent. Without tools the provider is chat only.
- Model lists are fetched live where the API allows (8 s timeout), with the configured
  `models` as the fallback (`models.rs`).
- HTTP: 30 s connect timeout, 300 s read timeout.

## Keys (`credentials.rs`)

- The env var named by `api_key_env` wins, then the OS keychain (service `cradle`,
  account = the env var name). Keys are never written to files.
- Keys are entered on the TUI setup screen, which saves them to the keychain. There is no
  `cradle auth` subcommand.
- `CRADLE_NO_KEYCHAIN=1` disables keychain lookups.
- Subscription OAuth tokens are never used here. Subscriptions go through the CLI backends.

## Open

- Whether to add the OpenAI Responses API adapter later, for full reasoning-model support.

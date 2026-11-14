# API backends (ModelBackend)

Cradle runs the agent loop itself and talks to HTTP APIs.

## Adapters

- **anthropic**: Messages API with streaming over SSE, tool use, `cache_control` prompt
  caching, and thinking blocks. Its message format is Cradle's canonical internal format.
- **openai_compatible**: Chat Completions. Covers OpenAI, OpenRouter, Ollama, LM Studio,
  Groq and others. Converts down from the canonical format.

## Decisions

- Capabilities from `providers.toml` decide whether tools, streaming, caching and reasoning
  parameters are sent.
- Keys: the env var (`api_key_env`) takes precedence over the OS keychain entry (via the
  `keyring` crate). There's a `cradle auth set <provider>` command to store a key.
- Subscription OAuth tokens are never used here. Subscriptions are only supported through
  the CLI backends.

## Open

- Whether to add the OpenAI Responses API adapter later, for full reasoning-model support.

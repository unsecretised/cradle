# providers.toml

Declare the providers to compile in from `providers.toml` at the workspace root, and let
users extend them at runtime.

## Decisions

- `cradle-backends/build.rs`:
  1. Reads `../../providers.toml` and prints `cargo:rerun-if-changed`.
  2. Validates it with `cradle-provider-spec`, which is a build dependency.
  3. Writes `OUT_DIR/providers.rs`.
- A thin macro pulls in the generated file:
  `macro_rules! providers { () => { include!(concat!(env!("OUT_DIR"), "/providers.rs")) } }`.
- Runtime: `~/.config/cradle/providers.toml` uses the same schema and is merged by `id`. A
  new `id` adds a provider, and an existing `id` overrides its fields.
- No secrets anywhere: only `api_key_env` / keychain names. The validator rejects values
  that look like keys.

## Schema

```toml
[[provider]]
id = "anthropic"
kind = "anthropic"            # anthropic | openai_compatible | claude_code | codex
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
auth = "key"                  # key | none
models = ["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]
default_model = "claude-sonnet-5-5"
headers = { }                 # optional extra headers
[provider.capabilities]       # optional; defaults depend on kind
tools = true
streaming = true
prompt_caching = true
reasoning = true

[[provider]]
id = "claude-code"
kind = "claude_code"
binary = "claude"
version = ">=2.0, <3.0"
```

## Validation (compile error / startup error)

- Duplicate `id`, unknown `kind`, a bad `base_url`, or a `default_model` that isn't in
  `models`.
- Fields that don't belong to the kind: `base_url`, keys and capabilities aren't allowed on
  CLI kinds, and HTTP kinds require `base_url`.
- `prompt_caching = true` on `openai_compatible`.
- Capability defaults: anthropic has everything on. openai_compatible has tools and
  streaming on, and caching and reasoning off.
- `tools = false` means the provider can only chat. The TUI must make this clear.

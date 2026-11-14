# providers.toml

Providers are declared in `providers.toml` at the workspace root, compiled in, and
extendable at runtime.

## Build

`cradle-backends/build.rs` reads `../../providers.toml` (with `rerun-if-changed`),
validates it with `cradle-provider-spec` (a build dependency) and writes
`OUT_DIR/providers.rs`. A `providers!()` macro includes it. Other crates call
`cradle_backends::builtin_providers()` or `providers(overlay)`. An invalid entry fails
the build.

## Runtime overlay

`~/.config/cradle/providers.toml` uses the same schema and is merged by `id`. A new `id` adds
a provider, and an existing `id` overrides it field by field (capabilities too). A
different `kind` replaces the entry entirely. The result is validated again. `cradle
providers` lists the merged set.

## Schema

```toml
[[provider]]
id = "anthropic"
kind = "anthropic"            # anthropic | openai_compatible | claude_code | codex
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"   # name of the env var / keychain entry
auth = "key"                  # key | none
models = ["claude-opus-5-5"]
default_model = "claude-opus-5-5"
headers = { }                 # optional; credentials are rejected
[provider.capabilities]       # optional; defaults depend on kind
tools = true
streaming = true
prompt_caching = true
reasoning = true

[[provider]]
id = "claude-code"
kind = "claude_code"
binary = "claude"             # default per kind
models = ["opus", "sonnet"]
version = ">=2.1, <3.0"       # semver range checked at startup
```

## Validation

- Duplicate `id`, unknown `kind`, bad `base_url`, `default_model` not in `models`.
- Fields that don't belong to the kind (`base_url`, keys and capabilities on CLI kinds).
  HTTP kinds require `base_url`.
- `prompt_caching = true` on `openai_compatible`.
- Values that look like secrets.
- Capability defaults: anthropic has everything on. openai_compatible has tools and
  streaming on, caching and reasoning off. `tools = false` means chat only.

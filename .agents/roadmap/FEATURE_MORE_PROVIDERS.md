# More providers

Add support for more providers. Most HTTP ones only need a `providers.toml` entry. CLI
agents need a new `kind` and adapter under `cli/`, like `claude_code` and `codex`.

**Everything below needs checking before it's added.** URLs, flags, MCP support and
subscription terms change often.

## Requirements for a CLI backend

A CLI can only be a backend if Cradle keeps control (see `features/FEATURE_CLI_BACKENDS.md`):
1. A headless / non-interactive mode with machine-readable output (JSON / stream-json).
2. **All built-in tools can be turned off** (shell, edit, web, subagents), so that only
   Cradle's tools are available.
3. It can load an MCP server from a config Cradle passes in (bearer token not on the
   command line), and ignores the project's own config and hooks.
4. Resume by session id (see `FEATURE_SESSION_RESUME.md`).
5. Using the vendor's subscription from a third-party harness is allowed under its
   terms. Cradle never reads OAuth tokens itself.

## Requested

| Provider | Path | Notes |
|---|---|---|
| **Grok (xAI)** | `openai_compatible`, `https://api.x.ai/v1`, `XAI_API_KEY` | Config only. Easiest one. |
| **OpenCode Go** | `opencode` CLI as a new `kind`, or its API endpoint if it has one | Subscription for open models. Check: an endpoint with a key, `opencode run` JSON output, turning off built-in tools, MCP config, `opencode serve`. |
| **Amp** (Sourcegraph) | new `amp` CLI kind | Check: execute mode with stream-json, turning off built-in tools, MCP config. Amp picks the model itself (no free model choice). |
| **Pi** (pi coding agent) | new `pi` CLI kind (JSON / RPC mode) | Pi works with several providers and subscription logins. Pi has (or had) **no MCP support on purpose**, so it would need a pi extension that bridges to Cradle's tools. Check before committing to it. |
| **Antigravity** (Google) | probably not possible | It's an IDE, and no headless CLI is known. For Google subscriptions, Gemini CLI below is the likely way. Revisit if a headless mode ships. |

## Also worth adding

### HTTP, config only (`openai_compatible`)

| Provider | Base URL (verify) | Key env |
|---|---|---|
| OpenAI | `https://api.openai.com/v1` | `OPENAI_API_KEY` (already commented out in `providers.toml`) |
| OpenRouter | `https://openrouter.ai/api/v1` | `OPENROUTER_API_KEY` (one key, many models) |
| Google Gemini | `https://generativelanguage.googleapis.com/v1beta/openai/` | `GEMINI_API_KEY` |
| Groq | `https://api.groq.com/openai/v1` | `GROQ_API_KEY` |
| Cerebras | `https://api.cerebras.ai/v1` | `CEREBRAS_API_KEY` |
| DeepSeek | `https://api.deepseek.com` | `DEEPSEEK_API_KEY` |
| Mistral | `https://api.mistral.ai/v1` | `MISTRAL_API_KEY` |
| Together | `https://api.together.xyz/v1` | `TOGETHER_API_KEY` |
| Fireworks | `https://api.fireworks.ai/inference/v1` | `FIREWORKS_API_KEY` |
| Moonshot (Kimi), Z.ai (GLM), MiniMax, Qwen (DashScope) | TODO | TODO. Some also offer Anthropic-compatible endpoints and coding plans. |
| LM Studio (local) | `http://localhost:1234/v1` | `auth = "none"` |
| llama.cpp server, vLLM (local) | `http://localhost:8080/v1`, `:8000/v1` | `auth = "none"` |

### Anthropic-compatible endpoints (`anthropic` kind, different `base_url`)

Some vendors (e.g. Kimi, GLM, MiniMax, DeepSeek) offer an Anthropic Messages endpoint.
That would give them thinking blocks and tool use through the existing adapter.
Check: prompt caching and thinking support, since `anthropic` turns every capability on
by default.

### HTTP, new adapter needed

- **OpenAI Responses API**: already an open question in `FEATURE_API_BACKENDS.md`. Full
  reasoning support for OpenAI models.
- **Azure OpenAI**: `api-key` header and `api-version` query parameter. Maybe just
  `headers` plus a URL template.
- **AWS Bedrock**: SigV4 signing, AWS credential chain.
- **Google Vertex AI**: OAuth / ADC credentials.

### CLI agents (subscription access)

| CLI | Why | Check |
|---|---|---|
| Gemini CLI (`gemini`) | Google subscription / free tier | headless JSON output, excluding all core tools, MCP via settings |
| GitHub Copilot CLI (`copilot`) | Copilot subscription, many models | headless mode, denying built-in tools, MCP |
| Cursor CLI (`cursor-agent`) | Cursor subscription | stream-json, MCP config not from the project, turning off tools |
| Qwen Code | Qwen free tier | fork of Gemini CLI, so it may share an adapter |
| Factory Droid, Kiro CLI, Goose, Crush | lower priority | same checklist |

## Suggested order

1. Config only: Grok, OpenRouter, Gemini, DeepSeek, Groq, Mistral, LM Studio (no code,
   just `providers.toml`).
2. Anthropic-compatible vendor endpoints.
3. One more CLI kind to make the `cli/` layer more general: Gemini CLI or OpenCode.
4. Amp, Copilot CLI, Cursor CLI.
5. New HTTP adapters (Responses, Bedrock, Vertex, Azure).
6. Pi (needs an extension bridge) and Antigravity (blocked on headless mode).

## Open

- Should config-only providers be compiled in (shown in the setup screen) or shipped as
  a commented example overlay, to keep the list short?
- A generic, configurable "CLI agent" kind, instead of one adapter per CLI?

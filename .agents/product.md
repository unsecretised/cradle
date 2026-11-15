# Cradle

Cradle is a terminal coding-agent harness written in Rust. It runs an LLM agent against a
project and keeps that agent inside rules the project and the user define.

## What makes it different

- **Project knowledge lives in `.agents/`** (or a single `agents.toml` for small projects)
  instead of one big `AGENTS.md`. Features, roadmap, bugfixes, build steps, tests,
  architecture, dependencies, rules and modes each have their own file.
- **Rules are enforced by the harness, not by asking the model nicely.** The agent has no raw
  shell. It only gets typed tools, and it can only run the commands the project lists in
  `[commands]`. Git use is limited to `git diff`, so Cradle never commits, branches, stashes
  or opens PRs on its own.
- **Project config has to be trusted.** Modes, commands and soul overrides from a project are
  shown to the user for approval, and approved again whenever they change.
- **Soul (Preferences).** User-level behaviour preferences: how often to ask questions (1-10),
  technical vs. non-technical audience, concise vs. detailed explanations, and verifying
  builds automatically. Set during setup or with `/preferences`. A project can suggest its
  own values; each one applies only if the user picks it.
- **Custom modes.** Plan and build are the defaults, and a project can add its own (e.g. a
  debug mode). `clash_handler` decides how project modes interact with the user's global
  modes. A built-in scaffold mode sets up `.agents/` for a new project.
- **Skills.** Reusable task instructions in `.agents/skills/`, listed to the agent and
  runnable with `/skill`.
- **Several backends.** Direct APIs (Anthropic, OpenAI-compatible) using API keys, plus the
  Claude Code and Codex CLIs, which let people use their existing subscriptions. Providers
  are declared in `providers.toml`, compiled in, and can be extended at runtime.

## Who it is for

Developers who want an agent that stays inside clearly defined limits, and who want project
knowledge kept in a structured form that both people and agents can maintain.

## Status

Pre-alpha, but usable: the workspace, TUI, all four backend kinds, trust, modes, sessions,
skills and preferences (Soul) are implemented (see `features.md`). Not started yet: a
command sandbox and the ACP front end (see `roadmap/`).

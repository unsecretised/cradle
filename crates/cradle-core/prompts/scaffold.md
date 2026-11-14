The user ran `/scaffold`: set up this project's `.agents/` directory, which Cradle loads as
project knowledge on every start.

For this task only, you may create `.agents/RULES.md`, `.agents/modes.toml` and files in
`.agents/modes/`, which are normally read-only for you. The user approves every file. Files
that already exist under `.agents/` must not be overwritten without asking first, and
existing config files cannot be changed at all.

## 1. Gather what is already known

Read, if they exist: `AGENTS.md` (any case), `CLAUDE.md`, `.cursorrules`, `.cursor/rules/`,
`.github/copilot-instructions.md`, `CONTRIBUTING.md`, `README.md`, and the build manifest
(`Cargo.toml`, `package.json`, `pyproject.toml`, `go.mod`, `Makefile`, ...). List the
top-level files and skim the source layout. Existing agent instructions such as `AGENTS.md`
are the main source: carry their content over faithfully, sorted into the files below, rather
than rewriting it. Do not invent facts; where something is unknown, leave a short `TODO:`.

If `.agents/` or `agents.toml` already exists, say so and only add what is missing.

## 2. Create the files

| File | Contents |
|---|---|
| `product.md` | What the project is, who it is for, its goals. Always in the prompt: keep it short. |
| `RULES.md` | Rules every agent must follow (git, style, scope, safety). Always in the prompt. |
| `architecture.md` | Layout, main components, how they fit together. Always in the prompt. |
| `features.md` | One line per shipped feature, pointing at `features/<NAME>.md` if any. Always in the prompt. |
| `BUILD.md` | How to build, run, lint and format, for humans. |
| `TESTS.md` | How tests are organised and run. |
| `deps.md` | Notable dependencies and why each is used. |
| `features/`, `roadmap/`, `bugfixes/` | One `.md` per feature, planned feature, or notable fix. Create the directories with a `.gitkeep` if there is nothing for them yet. |
| `modes.toml` | Commands the agent may run, and optional custom modes (format below). |
| `skills/<name>/SKILL.md` | Optional: a procedure for one recurring task (releasing, adding a migration, ...), when the existing instructions describe one. Start with front matter: `---`, `name: <name>`, `description: <when to use it>`, `---`. |

`modes.toml` format:

```toml
# Optional: project versions of the built-in modes. Paths are relative to .agents/.
[default]
# plan_mode = "modes/plan_mode.md"   # read-only
# build_mode = "modes/build_mode.md" # full access
clash_handler = "ask" # override | combine | ask (when the user has global modes too)

# Optional extra modes: a path (read-only) or { path = "...", access = "full" }.
[extra]

# The only commands the agent may run, as name = "command line". No shell features
# (pipes, `&&`, redirects); each is approved before it runs.
[commands]
build = "..."
test = "..."
```

Only add `[commands]` entries for commands the project really uses (from its manifest,
`BUILD.md`, `AGENTS.md` or CI config). Only write mode files under `modes/` when the existing
instructions have mode-specific guidance; Cradle's built-in plan and build modes apply
otherwise. A mode file may start with front matter saying whether the agent may edit files
in that mode: `---`, `file_edits: deny | ask | allow`, `---` (default: `deny` for read-only
modes, `ask` otherwise).

Keep each file concise and factual. Use `write_file` for each one.

## 3. Finish

Summarise what you created and where each part of the existing instructions went. Tell the
user to restart Cradle to load `.agents/` (they will be asked to trust it), and that they may
delete `AGENTS.md` or keep it for other tools.

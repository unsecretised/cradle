# Skills

Reusable instructions for recurring tasks (`cradle-config/src/skills.rs`).

- Location: `.agents/skills/<name>/SKILL.md` or `.agents/skills/<name>.md`, for both
  config forms.
- Optional front matter (`---` fenced `key: value`) gives `name` and `description`.
  Without it, the name is the directory or file name.
- The system prompt lists each skill with its description and path. The agent reads the
  file with `read_file` when a task matches.
- `/skill <name> [request]` (alias `/skills`) runs a skill explicitly. Skill names are
  suggested while typing.
- Skills are part of the trusted config snapshot, so adding or changing one asks for
  trust again.

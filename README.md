<!--This file is not to be edited by any agent. Strictly only for initial development and setup purposes-->
# Cradle coding agent harness:
- .agents/ directory instead of AGENTS.md, consisting off:
	- features/
		- FEATURE_XYZ.md
		- FEATURE_ABC.md
	- roadmap/
		- FEATURE_DEF.md
	- bugfixes/
		- BUGFIX_ABC.md
	- BUILD.md (how to build the code / run it / test it)
	- TESTS.md (what are the tests)
	- features.md (high level overview of each feature stored in featurees/)
	- product.md (description about the app / product)
	- architecture.md (how the code is structured)
	- deps.md (what are the dependencies and where to get their Docs, e.g. crates.io/crates/tokio)
	- RULES.md (what is allowed / not allowed)
	- modes.toml (see instructions on custom modes to understand format / purpose)
	
- or for smaller projects (use the example provided and use it to infer how the other sections could be represented):
	- agents.toml:
```toml
[features]
"abcd" = "allows user to do abcd"

[roadmap]
"xyz" = "let the user do xyz"

[architecture.src]
"main.rs" = "entrypoint"
"module.rs" = "modules"

[architecture.src.module]
"spliter.rs" = "split code"
"parser.rs" = "parse code"
```
- no automatic commiting / branch / stash / pr creation. Only git diff is allowed.
- Configurable "Soul" such as "Ask more questions" / "Ask fewer questions", "Verify builds automatically", "Ask techincal questions" vs "Assume no technical knowledge". Anything that is project configured must be run through by the user. (e.g. the "override" / crash_handler for custom modes)
- Custom Modes (default is plan and build mode) 
```toml
[default]
plan_mode = "modes/plan_mode.md" # not allowed to read files outside of .agents/ for modes, actual file path is .agents/modes/plan_mode.md
build_mode = "modes/build_mode.md" # same as plan mode
clash_handler = "override" # or combine or ask. override will use the project specific modes, combine will use both simulatenously, ask will ask the user. This clash is because there are also global settings for the harness that the user may have customised / which will be used.

[extra]
debug_mode = "modes/debug_mode.md
```

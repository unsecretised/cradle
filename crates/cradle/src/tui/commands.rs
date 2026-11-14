//! Slash commands: the registry, and suggestions shown while typing them.

pub struct CommandInfo {
    pub name: &'static str,
    /// Argument hint for help and suggestions ("" for none).
    pub args: &'static str,
    pub description: &'static str,
    /// The command does nothing useful without an argument.
    pub needs_arg: bool,
}

/// Every command, in the order shown. Aliases (`/models`, `/sessions`, `/resume`,
/// `/skills`, `/exit`) work but aren't listed.
pub const COMMANDS: &[CommandInfo] = &[
    CommandInfo {
        name: "help",
        args: "",
        description: "keys and commands",
        needs_arg: false,
    },
    CommandInfo {
        name: "clear",
        args: "",
        description: "start a new conversation",
        needs_arg: false,
    },
    CommandInfo {
        name: "mode",
        args: "[name]",
        description: "show or switch mode",
        needs_arg: false,
    },
    CommandInfo {
        name: "plan",
        args: "",
        description: "switch to plan mode (read-only)",
        needs_arg: false,
    },
    CommandInfo {
        name: "build",
        args: "",
        description: "switch to build mode",
        needs_arg: false,
    },
    CommandInfo {
        name: "model",
        args: "[name]",
        description: "pick or switch the model or provider",
        needs_arg: false,
    },
    CommandInfo {
        name: "review",
        args: "user|subagent",
        description: "who approves commands",
        needs_arg: true,
    },
    CommandInfo {
        name: "diff",
        args: "",
        description: "show uncommitted changes",
        needs_arg: false,
    },
    CommandInfo {
        name: "session",
        args: "",
        description: "resume an earlier conversation",
        needs_arg: false,
    },
    CommandInfo {
        name: "sidebar",
        args: "",
        description: "show or hide changed files and usage",
        needs_arg: false,
    },
    CommandInfo {
        name: "skill",
        args: "<name> [request]",
        description: "use a skill from .agents/skills/",
        needs_arg: false,
    },
    CommandInfo {
        name: "quit",
        args: "",
        description: "exit cradle",
        needs_arg: false,
    },
];

/// The canonical name for a command or alias.
pub fn canonical(name: &str) -> &str {
    match name {
        "models" => "model",
        "sessions" | "resume" => "session",
        "skills" => "skill",
        "exit" => "quit",
        other => other,
    }
}

/// What suggestions are drawn from.
pub struct Context<'a> {
    /// Skill names and descriptions.
    pub skills: &'a [(String, String)],
    /// Mode names and whether each is read-only.
    pub modes: &'a [(String, bool)],
    pub current_mode: &'a str,
    /// `provider/model` for every usable provider.
    pub models: &'a [String],
    /// The current `provider/model`.
    pub current_model: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub label: String,
    pub detail: String,
    /// The input after Tab.
    pub complete: String,
    /// What Enter runs, or `None` if an argument is still needed.
    pub run: Option<String>,
}

/// Suggestions for the current input, best matches first.
pub fn suggest(input: &str, ctx: &Context<'_>) -> Vec<Suggestion> {
    let Some(body) = input.strip_prefix('/') else {
        return Vec::new();
    };
    if body.contains('\n') {
        return Vec::new();
    }
    match body.split_once(' ') {
        None => suggest_commands(body),
        Some((name, rest)) => suggest_args(canonical(name), rest.trim_start(), ctx),
    }
}

fn suggest_commands(prefix: &str) -> Vec<Suggestion> {
    let prefix = prefix.to_lowercase();
    ranked(COMMANDS.iter(), |c| c.name, &prefix)
        .into_iter()
        .map(|c| {
            let takes_arg = !c.args.is_empty();
            Suggestion {
                label: if takes_arg {
                    format!("/{} {}", c.name, c.args)
                } else {
                    format!("/{}", c.name)
                },
                detail: c.description.to_owned(),
                complete: if takes_arg {
                    format!("/{} ", c.name)
                } else {
                    format!("/{}", c.name)
                },
                run: (!c.needs_arg).then(|| format!("/{}", c.name)),
            }
        })
        .collect()
}

fn suggest_args(command: &str, prefix: &str, ctx: &Context<'_>) -> Vec<Suggestion> {
    if prefix.contains(' ') {
        return Vec::new();
    }
    let options: Vec<(String, String)> = match command {
        "mode" => ctx
            .modes
            .iter()
            .map(|(name, read_only)| {
                let access = if *read_only {
                    "read-only"
                } else {
                    "full access"
                };
                let current = if name == ctx.current_mode {
                    " · current"
                } else {
                    ""
                };
                (name.clone(), format!("{access}{current}"))
            })
            .collect(),
        "model" => ctx
            .models
            .iter()
            .map(|model| {
                let mut detail = if model.ends_with("/default") {
                    "the CLI chooses".to_owned()
                } else {
                    String::new()
                };
                if model == ctx.current_model {
                    detail.push_str(if detail.is_empty() {
                        "current"
                    } else {
                        " · current"
                    });
                }
                (model.clone(), detail)
            })
            .collect(),
        "skill" => ctx.skills.to_vec(),
        "review" => vec![
            ("user".to_owned(), "you approve every command".to_owned()),
            (
                "subagent".to_owned(),
                "a reviewer approves safe commands; anything else goes to you".to_owned(),
            ),
        ],
        _ => Vec::new(),
    };
    let prefix = prefix.to_lowercase();
    ranked(options.iter(), |(name, _)| name.as_str(), &prefix)
        .into_iter()
        .map(|(name, detail)| {
            let line = format!("/{command} {name}");
            Suggestion {
                label: name.clone(),
                detail: detail.clone(),
                // A skill may be followed by a request.
                complete: if command == "skill" {
                    format!("{line} ")
                } else {
                    line.clone()
                },
                run: Some(line),
            }
        })
        .collect()
}

/// Items whose name starts with `prefix`, then those that merely contain it.
fn ranked<'a, T>(
    items: impl Iterator<Item = &'a T> + Clone,
    name: impl Fn(&T) -> &str,
    prefix: &str,
) -> Vec<&'a T>
where
    T: 'a,
{
    let lower = |item: &T| name(item).to_lowercase();
    let mut out: Vec<&T> = items
        .clone()
        .filter(|i| lower(i).starts_with(prefix))
        .collect();
    out.extend(items.filter(|i| {
        let n = lower(i);
        !n.starts_with(prefix) && n.contains(prefix)
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(modes: &'a [(String, bool)], models: &'a [String]) -> Context<'a> {
        Context {
            skills: &[],
            modes,
            current_mode: "plan",
            models,
            current_model: "cc/sonnet",
        }
    }

    fn labels(s: &[Suggestion]) -> Vec<&str> {
        s.iter().map(|s| s.label.as_str()).collect()
    }

    #[test]
    fn lists_and_filters_commands() {
        let c = ctx(&[], &[]);
        assert_eq!(suggest("/", &c).len(), COMMANDS.len());
        assert!(suggest("hello", &c).is_empty());
        let m = suggest("/mo", &c);
        assert_eq!(labels(&m), ["/mode [name]", "/model [name]"]);
        assert_eq!(m[0].complete, "/mode ");
        assert_eq!(m[0].run.as_deref(), Some("/mode"));
        // "contains" matches come after prefix matches.
        assert_eq!(labels(&suggest("/ui", &c)), ["/build", "/quit"]);
    }

    #[test]
    fn required_arguments_complete_instead_of_running() {
        let r = suggest("/rev", &ctx(&[], &[]));
        assert_eq!(r[0].complete, "/review ");
        assert_eq!(r[0].run, None);
    }

    #[test]
    fn suggests_modes_models_and_review_options() {
        let modes = vec![("plan".to_owned(), true), ("build".to_owned(), false)];
        let models = vec![
            "cc/default".to_owned(),
            "cc/opus".to_owned(),
            "cc/sonnet".to_owned(),
        ];
        let c = ctx(&modes, &models);

        let m = suggest("/mode ", &c);
        assert_eq!(labels(&m), ["plan", "build"]);
        assert_eq!(m[0].detail, "read-only · current");
        assert_eq!(m[1].run.as_deref(), Some("/mode build"));

        let m = suggest("/models so", &c);
        assert_eq!(labels(&m), ["cc/sonnet"], "aliases complete too");
        assert_eq!(m[0].run.as_deref(), Some("/model cc/sonnet"));
        assert_eq!(m[0].detail, "current");
        let m = suggest("/model ", &c);
        assert_eq!(labels(&m), ["cc/default", "cc/opus", "cc/sonnet"]);
        assert_eq!(m[0].detail, "the CLI chooses");

        assert_eq!(labels(&suggest("/review su", &c)), ["subagent"]);
        assert!(suggest("/review user extra", &c).is_empty());
    }
}

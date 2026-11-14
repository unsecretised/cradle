//! Tool definitions sent to the model, and parsing of the model's tool calls.

use serde::Deserialize;
use serde_json::{Value, json};

/// What a tool can do. Modes with read-only access only get [`Access::Read`] tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: String,
    pub input_schema: Value,
    pub access: Access,
}

/// A parsed, validated tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    ReadFile {
        path: String,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    ListFiles {
        path: Option<String>,
        glob: Option<String>,
    },
    Search {
        pattern: String,
        path: Option<String>,
        glob: Option<String>,
    },
    GitDiff {
        path: Option<String>,
        staged: bool,
    },
    AskUser {
        question: String,
        options: Vec<String>,
    },
    WriteFile {
        path: String,
        content: String,
    },
    EditFile {
        path: String,
        old_string: String,
        new_string: String,
        replace_all: bool,
    },
    RunCommand {
        name: String,
        reason: String,
    },
}

impl ToolCall {
    pub fn access(&self) -> Access {
        match self {
            ToolCall::ReadFile { .. }
            | ToolCall::ListFiles { .. }
            | ToolCall::Search { .. }
            | ToolCall::GitDiff { .. }
            | ToolCall::AskUser { .. } => Access::Read,
            ToolCall::WriteFile { .. }
            | ToolCall::EditFile { .. }
            | ToolCall::RunCommand { .. } => Access::Write,
        }
    }

    /// One-line description for the UI.
    pub fn summary(&self) -> String {
        match self {
            ToolCall::ReadFile { path, offset, .. } => match offset {
                Some(o) => format!("read {path} (from line {o})"),
                None => format!("read {path}"),
            },
            ToolCall::ListFiles { path, glob } => format!(
                "list {}{}",
                path.as_deref().unwrap_or("."),
                glob.as_ref()
                    .map(|g| format!(" matching {g}"))
                    .unwrap_or_default()
            ),
            ToolCall::Search { pattern, path, .. } => {
                format!("search /{pattern}/ in {}", path.as_deref().unwrap_or("."))
            }
            ToolCall::GitDiff { path, staged } => format!(
                "git diff{}{}",
                if *staged { " --cached" } else { "" },
                path.as_ref()
                    .map(|p| format!(" -- {p}"))
                    .unwrap_or_default()
            ),
            ToolCall::AskUser { question, .. } => format!("ask: {question}"),
            ToolCall::WriteFile { path, .. } => format!("write {path}"),
            ToolCall::EditFile { path, .. } => format!("edit {path}"),
            ToolCall::RunCommand { name, .. } => format!("run `{name}`"),
        }
    }
}

/// All tools, in a fixed order (stable order keeps the prompt cache warm).
pub fn definitions(commands: &[(&str, &str)]) -> Vec<ToolDefinition> {
    let command_list = if commands.is_empty() {
        "No commands are configured for this project.".to_owned()
    } else {
        commands
            .iter()
            .map(|(name, line)| format!("- `{name}`: {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let command_names: Vec<&str> = commands.iter().map(|(name, _)| *name).collect();

    vec![
        ToolDefinition {
            name: "read_file",
            description: "Read a UTF-8 text file in the project. Returns numbered lines. \
                          Use offset (1-based line) and limit to page through long files."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path relative to the project root"},
                    "offset": {"type": "integer", "minimum": 1},
                    "limit": {"type": "integer", "minimum": 1}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            access: Access::Read,
        },
        ToolDefinition {
            name: "list_files",
            description: "List files in the project (ignored and secret files are hidden). \
                          Optionally restrict to a directory and/or a glob such as `**/*.rs`."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Directory to list, relative to the root"},
                    "glob": {"type": "string"}
                },
                "additionalProperties": false
            }),
            access: Access::Read,
        },
        ToolDefinition {
            name: "search",
            description: "Search file contents with a regular expression (Rust regex syntax). \
                          Returns `path:line: text` for each match."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string"},
                    "path": {"type": "string", "description": "Directory or file to search"},
                    "glob": {"type": "string", "description": "Only search files matching this glob"}
                },
                "required": ["pattern"],
                "additionalProperties": false
            }),
            access: Access::Read,
        },
        ToolDefinition {
            name: "git_diff",
            description: "Show uncommitted changes (`git diff`). This is the only git \
                          operation available; never ask to commit, branch or push."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "staged": {"type": "boolean", "description": "Show staged changes instead"}
                },
                "additionalProperties": false
            }),
            access: Access::Read,
        },
        ToolDefinition {
            name: "ask_user",
            description: "Ask the user a question and wait for the answer. Use it when a \
                          decision is theirs to make. Offer options when there are clear choices."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "question": {"type": "string"},
                    "options": {"type": "array", "items": {"type": "string"}, "maxItems": 6}
                },
                "required": ["question"],
                "additionalProperties": false
            }),
            access: Access::Read,
        },
        ToolDefinition {
            name: "write_file",
            description: "Create a file or replace its entire content. Prefer edit_file for \
                          changes to existing files."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string", "description": "The full file content"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
            access: Access::Write,
        },
        ToolDefinition {
            name: "edit_file",
            description: "Replace an exact string in a file. old_string must match the file \
                          exactly (read it first) and be unique unless replace_all is true."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "old_string": {"type": "string"},
                    "new_string": {"type": "string"},
                    "replace_all": {"type": "boolean"}
                },
                "required": ["path", "old_string", "new_string"],
                "additionalProperties": false
            }),
            access: Access::Write,
        },
        ToolDefinition {
            name: "run_command",
            description: format!(
                "Run one of the project's configured commands by name. Arbitrary shell \
                     commands are not available. Each run is shown to the user or a reviewer \
                     for approval, so give a short reason.\n\nConfigured commands:\n{command_list}"
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "enum": command_names},
                    "reason": {"type": "string", "description": "Why you need to run it"}
                },
                "required": ["name", "reason"],
                "additionalProperties": false
            }),
            access: Access::Write,
        },
    ]
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadFileInput {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListFilesInput {
    path: Option<String>,
    glob: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitDiffInput {
    path: Option<String>,
    #[serde(default)]
    staged: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AskUserInput {
    question: String,
    #[serde(default)]
    options: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteFileInput {
    path: String,
    content: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditFileInput {
    path: String,
    old_string: String,
    new_string: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunCommandInput {
    name: String,
    reason: String,
}

/// Validate a tool call's input against its schema. The error is returned to the model.
pub fn parse_call(name: &str, input: &Value) -> Result<ToolCall, String> {
    fn de<T: serde::de::DeserializeOwned>(input: &Value) -> Result<T, String> {
        T::deserialize(input).map_err(|e| format!("invalid input: {e}"))
    }
    Ok(match name {
        "read_file" => {
            let i: ReadFileInput = de(input)?;
            ToolCall::ReadFile {
                path: i.path,
                offset: i.offset,
                limit: i.limit,
            }
        }
        "list_files" => {
            let i: ListFilesInput = de(input)?;
            ToolCall::ListFiles {
                path: i.path,
                glob: i.glob,
            }
        }
        "search" => {
            let i: SearchInput = de(input)?;
            ToolCall::Search {
                pattern: i.pattern,
                path: i.path,
                glob: i.glob,
            }
        }
        "git_diff" => {
            let i: GitDiffInput = de(input)?;
            ToolCall::GitDiff {
                path: i.path,
                staged: i.staged,
            }
        }
        "ask_user" => {
            let i: AskUserInput = de(input)?;
            ToolCall::AskUser {
                question: i.question,
                options: i.options,
            }
        }
        "write_file" => {
            let i: WriteFileInput = de(input)?;
            ToolCall::WriteFile {
                path: i.path,
                content: i.content,
            }
        }
        "edit_file" => {
            let i: EditFileInput = de(input)?;
            ToolCall::EditFile {
                path: i.path,
                old_string: i.old_string,
                new_string: i.new_string,
                replace_all: i.replace_all,
            }
        }
        "run_command" => {
            let i: RunCommandInput = de(input)?;
            ToolCall::RunCommand {
                name: i.name,
                reason: i.reason,
            }
        }
        other => return Err(format!("unknown tool `{other}`")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_validates_calls() {
        assert_eq!(
            parse_call("read_file", &json!({"path": "a.rs"})).unwrap(),
            ToolCall::ReadFile {
                path: "a.rs".into(),
                offset: None,
                limit: None
            }
        );
        assert!(parse_call("read_file", &json!({})).is_err());
        assert!(parse_call("read_file", &json!({"path": "a", "extra": 1})).is_err());
        assert!(parse_call("bash", &json!({})).is_err());
    }

    #[test]
    fn definitions_are_stable_and_list_commands() {
        let defs = definitions(&[("test", "cargo test")]);
        let names: Vec<_> = defs.iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            [
                "read_file",
                "list_files",
                "search",
                "git_diff",
                "ask_user",
                "write_file",
                "edit_file",
                "run_command"
            ]
        );
        assert!(defs[7].description.contains("`test`: cargo test"));
    }
}

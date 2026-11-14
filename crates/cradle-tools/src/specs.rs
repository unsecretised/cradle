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

/// The most files one `read_files` call may read.
pub const MAX_READ_FILES: usize = 10;
/// The most lines `read_files` returns per file.
pub const MAX_LINES_PER_FILE: usize = 2000;

/// One file to read in a `read_files` call. `offset` is 1-based.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileRange {
    pub path: String,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

/// The fewest options a choice question may offer.
pub const MIN_OPTIONS: usize = 2;
/// The most options a choice question may offer (shortcuts 1-9 and 0).
pub const MAX_OPTIONS: usize = 10;

/// How the user answers a question in an `ask_user` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuestionKind {
    /// Pick one option (or type an own answer).
    Single,
    /// Pick one or more options (and/or type an own answer).
    Multi,
    /// A short free-text answer.
    Text,
}

/// One question in an `ask_user` call, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub question: String,
    pub kind: QuestionKind,
    /// Empty for [`QuestionKind::Text`].
    pub options: Vec<String>,
}

impl Question {
    /// A single-choice question.
    pub fn single(question: impl Into<String>, options: Vec<String>) -> Self {
        Question {
            question: question.into(),
            kind: QuestionKind::Single,
            options,
        }
    }
}

/// What a tool call does, in a form the UI can count and group without parsing summaries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Read,
    List,
    Search,
    Diff,
    Ask,
    Write,
    Edit,
    Command,
}

impl ActivityKind {
    /// Whether consecutive calls of this kind are shown together as one group.
    pub fn groups(self) -> bool {
        matches!(
            self,
            ActivityKind::Read | ActivityKind::List | ActivityKind::Search
        )
    }

    /// A lowercase phrase for `count` items, e.g. "read 3 files".
    pub fn phrase(self, count: usize) -> String {
        let (verb, one, many) = match self {
            ActivityKind::Read => ("read", "file", "files"),
            ActivityKind::List => ("listed", "directory", "directories"),
            ActivityKind::Search => ("searched", "pattern", "patterns"),
            ActivityKind::Diff => ("showed", "diff", "diffs"),
            ActivityKind::Ask => ("asked", "question", "questions"),
            ActivityKind::Write => ("wrote", "file", "files"),
            ActivityKind::Edit => ("edited", "file", "files"),
            ActivityKind::Command => ("ran", "command", "commands"),
        };
        format!("{verb} {count} {}", if count == 1 { one } else { many })
    }
}

/// A tool call's kind, how many items it covers, and one detail line per item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolActivity {
    pub kind: ActivityKind,
    pub count: usize,
    pub details: Vec<String>,
}

impl ToolActivity {
    fn one(kind: ActivityKind, detail: String) -> Self {
        ToolActivity {
            kind,
            count: 1,
            details: vec![detail],
        }
    }
}

/// A parsed, validated tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCall {
    ReadFile {
        path: String,
        offset: Option<usize>,
        limit: Option<usize>,
    },
    ReadFiles {
        files: Vec<FileRange>,
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
        questions: Vec<Question>,
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
            | ToolCall::ReadFiles { .. }
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
            ToolCall::ReadFiles { files } => match files.as_slice() {
                [only] => format!("read {}", only.path),
                files => format!("read {} files", files.len()),
            },
            ToolCall::ListFiles { path, glob } => format!("list {}", list_target(path, glob)),
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
            ToolCall::AskUser { questions } => match questions.as_slice() {
                [only] => format!("ask: {}", only.question),
                qs => format!("ask {} questions", qs.len()),
            },
            ToolCall::WriteFile { path, .. } => format!("write {path}"),
            ToolCall::EditFile { path, .. } => format!("edit {path}"),
            ToolCall::RunCommand { name, .. } => format!("run `{name}`"),
        }
    }

    /// What this call does, for grouping and counting in the UI.
    pub fn activity(&self) -> ToolActivity {
        use ActivityKind as K;
        match self {
            ToolCall::ReadFile { path, .. } => ToolActivity::one(K::Read, path.clone()),
            ToolCall::ReadFiles { files } => ToolActivity {
                kind: K::Read,
                count: files.len(),
                details: files.iter().map(|f| f.path.clone()).collect(),
            },
            ToolCall::ListFiles { path, glob } => {
                ToolActivity::one(K::List, list_target(path, glob))
            }
            ToolCall::Search {
                pattern,
                path,
                glob,
            } => ToolActivity::one(
                K::Search,
                format!(
                    "/{pattern}/ in {}{}",
                    path.as_deref().unwrap_or("."),
                    glob.as_ref()
                        .map(|g| format!(" matching {g}"))
                        .unwrap_or_default()
                ),
            ),
            ToolCall::GitDiff { .. } => ToolActivity::one(K::Diff, self.summary()),
            ToolCall::AskUser { questions } => ToolActivity {
                kind: K::Ask,
                count: questions.len(),
                details: questions.iter().map(|q| q.question.clone()).collect(),
            },
            ToolCall::WriteFile { path, .. } => ToolActivity::one(K::Write, path.clone()),
            ToolCall::EditFile { path, .. } => ToolActivity::one(K::Edit, path.clone()),
            ToolCall::RunCommand { name, .. } => ToolActivity::one(K::Command, name.clone()),
        }
    }
}

fn list_target(path: &Option<String>, glob: &Option<String>) -> String {
    format!(
        "{}{}",
        path.as_deref().unwrap_or("."),
        glob.as_ref()
            .map(|g| format!(" matching {g}"))
            .unwrap_or_default()
    )
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
            name: "read_files",
            description: format!(
                "Read several UTF-8 text files in one call (up to {MAX_READ_FILES}). Prefer \
                 this over repeated read_file calls when you need more than one file. Each \
                 file is returned under a `==> path <==` header with numbered lines, at most \
                 {MAX_LINES_PER_FILE} lines per file. A file that cannot be read gets an error \
                 in its section; the others are still returned."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_READ_FILES,
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": {"type": "string", "description": "Path relative to the project root"},
                                "offset": {"type": "integer", "minimum": 1},
                                "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LINES_PER_FILE}
                            },
                            "required": ["path"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["files"],
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
            description: format!(
                "Ask the user one or more questions and wait for the answers. Use it when a \
                 decision is theirs to make, and put related questions in one call. They are \
                 shown one at a time. Each question has a kind: `single` (pick one of the \
                 options), `multi` (pick any number of the options) or `text` (a short \
                 free-text answer, no options). Choice questions take {MIN_OPTIONS}-{MAX_OPTIONS} \
                 options; the user may also type their own answer instead. `kind` defaults \
                 to `single` when options are given, else `text`."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {"type": "string"},
                                "kind": {"type": "string", "enum": ["single", "multi", "text"]},
                                "options": {
                                    "type": "array",
                                    "items": {"type": "string"},
                                    "maxItems": MAX_OPTIONS
                                }
                            },
                            "required": ["question"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["questions"],
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
struct ReadFilesInput {
    files: Vec<FileRange>,
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
    questions: Vec<QuestionInput>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuestionInput {
    question: String,
    kind: Option<QuestionKind>,
    #[serde(default)]
    options: Vec<String>,
}

impl QuestionInput {
    /// Resolve the default kind and check the options. `n` is the 1-based position.
    fn validate(self, n: usize) -> Result<Question, String> {
        if self.question.trim().is_empty() {
            return Err(format!("invalid input: question {n} is empty"));
        }
        let kind = self.kind.unwrap_or(if self.options.is_empty() {
            QuestionKind::Text
        } else {
            QuestionKind::Single
        });
        match kind {
            QuestionKind::Text if !self.options.is_empty() => Err(format!(
                "invalid input: question {n} is a `text` question and takes no options"
            )),
            QuestionKind::Single | QuestionKind::Multi
                if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&self.options.len()) =>
            {
                Err(format!(
                    "invalid input: question {n} needs {MIN_OPTIONS}-{MAX_OPTIONS} options \
                     (got {})",
                    self.options.len()
                ))
            }
            _ => Ok(Question {
                question: self.question,
                kind,
                options: self.options,
            }),
        }
    }
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
        "read_files" => {
            let i: ReadFilesInput = de(input)?;
            if i.files.is_empty() {
                return Err("invalid input: `files` must list at least one file".into());
            }
            if i.files.len() > MAX_READ_FILES {
                return Err(format!(
                    "invalid input: at most {MAX_READ_FILES} files per call (got {}); split \
                     the read into several calls",
                    i.files.len()
                ));
            }
            ToolCall::ReadFiles { files: i.files }
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
            if i.questions.is_empty() {
                return Err("invalid input: `questions` must list at least one question".into());
            }
            let questions = i
                .questions
                .into_iter()
                .enumerate()
                .map(|(n, q)| q.validate(n + 1))
                .collect::<Result<_, _>>()?;
            ToolCall::AskUser { questions }
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
                "read_files",
                "list_files",
                "search",
                "git_diff",
                "ask_user",
                "write_file",
                "edit_file",
                "run_command"
            ]
        );
        assert!(defs[8].description.contains("`test`: cargo test"));
    }

    #[test]
    fn parses_and_limits_read_files() {
        let call = parse_call(
            "read_files",
            &json!({"files": [{"path": "a.rs"}, {"path": "b.rs", "offset": 3, "limit": 10}]}),
        )
        .unwrap();
        assert_eq!(
            call,
            ToolCall::ReadFiles {
                files: vec![
                    FileRange {
                        path: "a.rs".into(),
                        offset: None,
                        limit: None
                    },
                    FileRange {
                        path: "b.rs".into(),
                        offset: Some(3),
                        limit: Some(10)
                    },
                ]
            }
        );
        assert_eq!(call.access(), Access::Read);
        assert_eq!(call.summary(), "read 2 files");

        assert!(parse_call("read_files", &json!({"files": []})).is_err());
        assert!(parse_call("read_files", &json!({"paths": ["a"]})).is_err());
        assert!(parse_call("read_files", &json!({"files": [{"path": "a", "x": 1}]})).is_err());
        let eleven: Vec<Value> = (0..11)
            .map(|i| json!({"path": format!("{i}.rs")}))
            .collect();
        let err = parse_call("read_files", &json!({ "files": eleven })).unwrap_err();
        assert!(err.contains("at most 10"), "{err}");
    }

    #[test]
    fn activities_describe_calls() {
        let read = parse_call(
            "read_files",
            &json!({"files": [{"path": "a"}, {"path": "b"}]}),
        )
        .unwrap()
        .activity();
        assert_eq!(read.kind, ActivityKind::Read);
        assert_eq!(
            (read.count, read.details),
            (2, vec!["a".into(), "b".into()])
        );

        let search = parse_call("search", &json!({"pattern": "fn", "path": "src"}))
            .unwrap()
            .activity();
        assert_eq!(search.kind, ActivityKind::Search);
        assert_eq!(search.details, ["/fn/ in src"]);

        let list = parse_call("list_files", &json!({})).unwrap().activity();
        assert_eq!((list.kind, list.count), (ActivityKind::List, 1));

        let edit = parse_call(
            "edit_file",
            &json!({"path": "a", "old_string": "x", "new_string": "y"}),
        )
        .unwrap()
        .activity();
        assert!(!edit.kind.groups());
        assert!(read.kind.groups() && search.kind.groups() && list.kind.groups());
    }

    #[test]
    fn parses_question_kinds_and_defaults() {
        let call = parse_call(
            "ask_user",
            &json!({"questions": [
                {"question": "Which?", "options": ["a", "b"]},
                {"question": "Which ones?", "kind": "multi", "options": ["a", "b", "c"]},
                {"question": "Why?"},
                {"question": "Name?", "kind": "text"}
            ]}),
        )
        .unwrap();
        let ToolCall::AskUser { questions } = &call else {
            panic!("{call:?}")
        };
        let kinds: Vec<_> = questions.iter().map(|q| q.kind).collect();
        assert_eq!(
            kinds,
            [
                QuestionKind::Single,
                QuestionKind::Multi,
                QuestionKind::Text,
                QuestionKind::Text
            ]
        );
        assert_eq!(questions[1].options, ["a", "b", "c"]);
        assert_eq!(call.access(), Access::Read);
        assert_eq!(call.summary(), "ask 4 questions");
        let activity = call.activity();
        assert_eq!((activity.kind, activity.count), (ActivityKind::Ask, 4));
        assert_eq!(activity.details[2], "Why?");

        let one = parse_call("ask_user", &json!({"questions": [{"question": "Ok?"}]})).unwrap();
        assert_eq!(one.summary(), "ask: Ok?");
    }

    #[test]
    fn has_no_question_limit() {
        let many: Vec<Value> = (0..25)
            .map(|i| json!({"question": format!("q{i}")}))
            .collect();
        let call = parse_call("ask_user", &json!({ "questions": many })).unwrap();
        assert_eq!(call.activity().count, 25);
    }

    #[test]
    fn rejects_bad_questions() {
        let ask = |q: Value| parse_call("ask_user", &json!({ "questions": [q] }));
        let ten: Vec<String> = (0..10).map(|i| i.to_string()).collect();
        let eleven: Vec<String> = (0..11).map(|i| i.to_string()).collect();
        assert!(ask(json!({"question": "q", "options": ten})).is_ok());
        let err = ask(json!({"question": "q", "options": eleven})).unwrap_err();
        assert!(err.contains("2-10 options"), "{err}");
        assert!(ask(json!({"question": "q", "kind": "single", "options": ["a"]})).is_err());
        assert!(ask(json!({"question": "q", "kind": "multi"})).is_err());
        let err = ask(json!({"question": "q", "kind": "text", "options": ["a", "b"]})).unwrap_err();
        assert!(err.contains("takes no options"), "{err}");
        assert!(ask(json!({"question": "q", "kind": "radio"})).is_err());
        assert!(ask(json!({"question": "  "})).is_err());
        assert!(ask(json!({"question": "q", "extra": 1})).is_err());
        assert!(parse_call("ask_user", &json!({"questions": []})).is_err());
        // The old single-question form is gone.
        assert!(parse_call("ask_user", &json!({"question": "q", "options": ["a"]})).is_err());
    }

    #[test]
    fn phrases_are_singular_or_plural() {
        assert_eq!(ActivityKind::Read.phrase(1), "read 1 file");
        assert_eq!(ActivityKind::Read.phrase(3), "read 3 files");
        assert_eq!(ActivityKind::Search.phrase(2), "searched 2 patterns");
        assert_eq!(ActivityKind::List.phrase(1), "listed 1 directory");
    }
}

//! The agent loop. Runs on its own task and talks to the UI over channels.
//!
//! Enforcement lives here, not in the prompt:
//! - Read-only modes refuse write tools whatever the model asks for.
//! - Writes to protected files and `.agents/` docs need the user's approval (with a diff).
//! - Every `run_command` is approved by the user, or by the reviewer subagent when that
//!   policy is on. The reviewer is skipped (the user decides) once any protected file has
//!   changed this session.
//! - History is append-only, which the API's thinking blocks require.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use cradle_backends::{
    AgentTurn, Backend, Block, McpEndpoint, Message, ModelRequest, Role, StopReason, StreamEvent,
    ToolSpec, Usage,
};
use cradle_config::{ModeAccess, ProjectConfig};
use cradle_mcp::{McpServer, McpTool, ToolReply, ToolRequest};
use cradle_tools::hygiene::reveal_invisible;
use cradle_tools::{Access, PathGuard, ToolCall, ToolDefinition, ToolError, WriteClass, command};
use serde_json::Value;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::modes::ModeSet;
use crate::prompt;
use crate::review::{self, ChangedFile, Verdict};
use crate::session::SessionLog;

const MAX_TOKENS: u32 = 64_000;
const PREVIEW_LINES: usize = 4;

/// Who approves `run_command`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewPolicy {
    /// Every command is shown to the user.
    User,
    /// A fresh reviewer model approves safe commands; anything else goes to the user.
    Subagent,
}

#[derive(Debug)]
pub enum AgentEvent {
    Text(String),
    Thinking(String),
    ToolStarted {
        id: String,
        summary: String,
    },
    ToolFinished {
        id: String,
        ok: bool,
        preview: String,
    },
    Diff {
        title: String,
        diff: String,
    },
    ReviewResult {
        command: String,
        verdict: Verdict,
        reason: String,
    },
    Approval(ApprovalRequest),
    Question(QuestionRequest),
    ModeChanged {
        name: String,
        read_only: bool,
    },
    ModelChanged {
        provider: String,
        model: String,
    },
    ReviewChanged(ReviewPolicy),
    Usage(Usage),
    Notice(String),
    Error(String),
    TurnFinished,
}

#[derive(Debug)]
pub struct ApprovalRequest {
    pub title: String,
    pub body: String,
    pub note: Option<String>,
    /// Whether "always allow this for the session" is offered.
    pub allow_always: bool,
    pub reply: oneshot::Sender<Approval>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Yes,
    Always,
    No,
}

#[derive(Debug)]
pub struct QuestionRequest {
    pub question: String,
    pub options: Vec<String>,
    pub reply: oneshot::Sender<String>,
}

#[derive(Debug)]
pub enum AgentCommand {
    Send(String),
    SetMode(String),
    CycleMode,
    SetReview(ReviewPolicy),
    ShowDiff,
    Clear,
    /// Switch model (same provider). The conversation continues on the new model.
    SetBackend(Box<Backend>),
}

pub struct AgentSetup {
    pub backend: Backend,
    pub guard: PathGuard,
    /// `None` when the project config is not trusted.
    pub project: Option<ProjectConfig>,
    pub modes: ModeSet,
    pub mode: String,
    pub review: ReviewPolicy,
    /// Messages from a resumed session.
    pub history: Vec<Message>,
    pub session: Option<SessionLog>,
    /// Where `/clear` starts a new transcript.
    pub data_dir: Option<PathBuf>,
}

pub struct AgentHandle {
    pub commands: mpsc::UnboundedSender<AgentCommand>,
    pub events: mpsc::UnboundedReceiver<AgentEvent>,
    cancel: Arc<Notify>,
}

impl AgentHandle {
    /// Stop the current model call, approval or tool. No effect when idle.
    pub fn cancel(&self) {
        self.cancel.notify_waiters();
    }
}

pub fn spawn(setup: AgentSetup) -> AgentHandle {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let cancel = Arc::new(Notify::new());
    let agent = Agent::new(setup, event_tx, cancel.clone());
    tokio::spawn(agent.run(command_rx));
    AgentHandle {
        commands: command_tx,
        events: event_rx,
        cancel,
    }
}

struct Agent {
    backend: Arc<Backend>,
    guard: PathGuard,
    system: String,
    tools: Vec<ToolDefinition>,
    tool_specs: Vec<ToolSpec>,
    commands: BTreeMap<String, String>,
    modes: ModeSet,
    mode: String,
    announced_mode: Option<String>,
    review: ReviewPolicy,
    history: Vec<Message>,
    session: Option<SessionLog>,
    data_dir: Option<PathBuf>,
    usage: Usage,
    /// Commands the user allowed for the rest of the session.
    always_allowed: HashSet<String>,
    /// Content of each changed file before its first change this session (`None`: new).
    originals: BTreeMap<String, (PathBuf, Option<String>)>,
    /// Protected files changed since the user last approved a command.
    protected_changed: BTreeSet<String>,
    /// For CLI backends: Cradle's MCP tool server and the calls it receives.
    mcp: Option<(McpServer, mpsc::UnboundedReceiver<ToolRequest>)>,
    /// For CLI backends: the CLI's own session id, to continue the conversation.
    cli_session: Option<String>,
    events: mpsc::UnboundedSender<AgentEvent>,
    cancel: Arc<Notify>,
}

impl Agent {
    fn new(
        setup: AgentSetup,
        events: mpsc::UnboundedSender<AgentEvent>,
        cancel: Arc<Notify>,
    ) -> Self {
        let commands = setup
            .project
            .as_ref()
            .map(|p| p.commands.clone())
            .unwrap_or_default();
        let command_list: Vec<(&str, &str)> = commands
            .iter()
            .map(|(n, l)| (n.as_str(), l.as_str()))
            .collect();
        let tools = cradle_tools::definitions(&command_list);
        let tool_specs = tools
            .iter()
            .map(|t| ToolSpec {
                name: t.name.to_owned(),
                description: t.description.clone(),
                input_schema: t.input_schema.clone(),
            })
            .collect();
        let system = prompt::system_prompt(setup.guard.root(), setup.project.as_ref());

        let mut history = setup.history;
        close_dangling_tool_calls(&mut history);

        Agent {
            backend: Arc::new(setup.backend),
            guard: setup.guard,
            system,
            tools,
            tool_specs,
            commands,
            modes: setup.modes,
            mode: setup.mode,
            announced_mode: None,
            review: setup.review,
            history,
            session: setup.session,
            data_dir: setup.data_dir,
            usage: Usage::default(),
            always_allowed: HashSet::new(),
            originals: BTreeMap::new(),
            protected_changed: BTreeSet::new(),
            mcp: None,
            cli_session: None,
            events,
            cancel,
        }
    }

    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<AgentCommand>) {
        self.emit_mode();
        self.emit(AgentEvent::ReviewChanged(self.review));
        if self.backend.is_cli() {
            let (tx, rx) = mpsc::unbounded_channel();
            let tools = self
                .tools
                .iter()
                .map(|t| McpTool {
                    name: t.name.to_owned(),
                    description: t.description.clone(),
                    input_schema: t.input_schema.clone(),
                    read_only: t.access == Access::Read,
                })
                .collect();
            match cradle_mcp::start(tools, tx).await {
                Ok(server) => self.mcp = Some((server, rx)),
                Err(e) => self.emit(AgentEvent::Error(format!(
                    "cannot start Cradle's tool server: {e}"
                ))),
            }
            if !self.history.is_empty() {
                self.emit(AgentEvent::Notice(format!(
                    "{} starts a fresh session: the resumed messages are shown but not sent.",
                    self.backend.provider()
                )));
            }
        }
        if !self.backend.supports_tools() {
            self.emit(AgentEvent::Notice(format!(
                "{} does not support tools: chat only, no file access.",
                self.backend.provider()
            )));
        }
        while let Some(command) = commands.recv().await {
            match command {
                AgentCommand::Send(text) => {
                    self.run_turn(text).await;
                    self.emit(AgentEvent::TurnFinished);
                }
                AgentCommand::SetMode(name) => {
                    if self.modes.get(&name).is_some() {
                        self.mode = name;
                        self.emit_mode();
                    } else {
                        let names: Vec<&str> = self.modes.names().collect();
                        self.emit(AgentEvent::Error(format!(
                            "unknown mode `{name}` (available: {})",
                            names.join(", ")
                        )));
                    }
                }
                AgentCommand::CycleMode => {
                    self.mode = self.modes.next_after(&self.mode).name.clone();
                    self.emit_mode();
                }
                AgentCommand::SetReview(policy) => {
                    self.review = policy;
                    self.emit(AgentEvent::ReviewChanged(policy));
                }
                AgentCommand::ShowDiff => {
                    match command::git_diff(self.guard.root(), None, false).await {
                        Ok(diff) => self.emit(AgentEvent::Diff {
                            title: "git diff".into(),
                            diff,
                        }),
                        Err(e) => self.emit(AgentEvent::Error(e.0)),
                    }
                }
                AgentCommand::SetBackend(backend) => {
                    if backend.is_cli() != self.backend.is_cli() {
                        self.emit(AgentEvent::Error(
                            "Switching between a CLI and an API provider needs a restart.".into(),
                        ));
                        continue;
                    }
                    self.backend = Arc::new(*backend);
                    self.emit(AgentEvent::ModelChanged {
                        provider: self.backend.provider().to_owned(),
                        model: self.backend.model().to_owned(),
                    });
                }
                AgentCommand::Clear => {
                    self.history.clear();
                    self.announced_mode = None;
                    self.cli_session = None;
                    if let (Some(dir), Some(_)) = (&self.data_dir, &self.session) {
                        self.session = SessionLog::create(dir, self.guard.root()).ok();
                    }
                    self.emit(AgentEvent::Notice("Started a new conversation.".into()));
                }
            }
        }
    }

    /// Announce the current mode if the model has not seen it yet. Returns the message.
    fn announce_mode(&mut self) -> Option<String> {
        if self.announced_mode.as_deref() == Some(self.mode.as_str()) {
            return None;
        }
        let mode = self.modes.get(&self.mode).expect("current mode exists");
        let message = prompt::mode_message(mode, &self.tools);
        self.push(Message::system(message.clone()));
        self.announced_mode = Some(self.mode.clone());
        Some(message)
    }

    /// A turn through a CLI backend: the CLI runs the loop, and its tool calls arrive on
    /// the MCP server while it runs. Each goes through the same checks as API turns.
    async fn run_cli_turn(&mut self, text: String) {
        self.push(Message::user(text.clone()));
        let mut prompt = String::new();
        if let Some(notice) = self.announce_mode() {
            prompt = format!("<harness_notice>\n{notice}\n</harness_notice>\n\n");
        }
        prompt.push_str(&text);

        let Some((server, mut calls)) = self.mcp.take() else {
            self.emit(AgentEvent::Error(
                "Cradle's tool server is not running.".into(),
            ));
            return;
        };
        // Calls from an earlier, cancelled turn have nobody waiting for them.
        while calls.try_recv().is_ok() {}

        let turn = AgentTurn {
            system: self.system.clone(),
            prompt,
            resume: self.cli_session.clone(),
            mcp: McpEndpoint {
                url: server.url().to_owned(),
                token: server.token().to_owned(),
            },
            root: self.guard.root().to_path_buf(),
        };
        let backend = self.backend.clone();
        let events = self.events.clone();
        let mut run = Box::pin(async move {
            let mut sink = move |event: StreamEvent| {
                let _ = match event {
                    StreamEvent::TextDelta(t) => events.send(AgentEvent::Text(t)),
                    StreamEvent::ThinkingDelta(t) => events.send(AgentEvent::Thinking(t)),
                    StreamEvent::ToolUseStarted { .. } => Ok(()),
                };
            };
            backend.run_agent_turn(&turn, &mut sink).await
        });

        let cancel = self.cancel.clone();
        let mut call_count = 0;
        let outcome = loop {
            tokio::select! {
                result = &mut run => break Some(result),
                Some(call) = calls.recv() => {
                    call_count += 1;
                    let id = format!("mcp-{call_count}");
                    let result = tokio::select! {
                        result = self.execute(&id, &call.name, &call.arguments, None) => Some(result),
                        _ = cancel.notified() => None,
                    };
                    let Some((content, is_error)) = result else {
                        self.emit(AgentEvent::ToolFinished {
                            id,
                            ok: false,
                            preview: "Cancelled.".into(),
                        });
                        break None;
                    };
                    self.emit(AgentEvent::ToolFinished {
                        id,
                        ok: !is_error,
                        preview: preview(&content),
                    });
                    let _ = call.reply.send(ToolReply { text: content, is_error });
                }
                _ = cancel.notified() => break None,
            }
        };
        // Dropping the future kills the CLI if it is still running.
        drop(run);
        self.mcp = Some((server, calls));

        match outcome {
            None => self.emit(AgentEvent::Notice("Cancelled.".into())),
            Some(Err(e)) => self.emit(AgentEvent::Error(e.to_string())),
            Some(Ok(outcome)) => {
                if outcome.session_id.is_some() {
                    self.cli_session = outcome.session_id;
                }
                self.usage += outcome.usage;
                self.emit(AgentEvent::Usage(self.usage));
                self.push(Message {
                    role: Role::Assistant,
                    content: vec![Block::Text { text: outcome.text }],
                });
            }
        }
    }

    async fn run_turn(&mut self, text: String) {
        if self.backend.is_cli() {
            return self.run_cli_turn(text).await;
        }
        self.push(Message::user(text));
        self.announce_mode();

        let cancel = self.cancel.clone();
        loop {
            let tools: &[ToolSpec] = if self.backend.supports_tools() {
                &self.tool_specs
            } else {
                &[]
            };
            let req = ModelRequest {
                system: &self.system,
                messages: &self.history,
                tools,
                max_tokens: MAX_TOKENS,
            };
            let events = self.events.clone();
            let mut sink = move |event: StreamEvent| {
                let _ = match event {
                    StreamEvent::TextDelta(t) => events.send(AgentEvent::Text(t)),
                    StreamEvent::ThinkingDelta(t) => events.send(AgentEvent::Thinking(t)),
                    StreamEvent::ToolUseStarted { .. } => Ok(()),
                };
            };
            let result = tokio::select! {
                result = self.backend.complete(&req, &mut sink) => result,
                _ = cancel.notified() => {
                    self.emit(AgentEvent::Notice("Cancelled.".into()));
                    return;
                }
            };
            let turn = match result {
                Ok(turn) => turn,
                Err(e) => {
                    self.emit(AgentEvent::Error(e.to_string()));
                    return;
                }
            };
            self.usage += turn.usage;
            self.emit(AgentEvent::Usage(self.usage));

            let calls: Vec<(String, String, Value, Option<String>)> = turn
                .message
                .tool_uses()
                .map(|c| {
                    (
                        c.id.to_owned(),
                        c.name.to_owned(),
                        c.input.clone(),
                        c.invalid_json.map(str::to_owned),
                    )
                })
                .collect();
            self.push(turn.message);

            if calls.is_empty() {
                match turn.stop {
                    StopReason::PauseTurn => continue,
                    StopReason::Refusal { .. } => self.emit(AgentEvent::Error(refusal(&turn.stop))),
                    StopReason::MaxTokens => self.emit(AgentEvent::Notice(
                        "The response was cut off (max tokens).".into(),
                    )),
                    _ => {}
                }
                return;
            }

            // Tool input in a cut-off or refused response may be truncated: never run it.
            let blocked = match &turn.stop {
                StopReason::MaxTokens => Some(
                    "Not run: the response was cut off before the tool input was complete. \
                     Retry in smaller steps.",
                ),
                StopReason::Refusal { .. } => Some("Not run: the response was declined."),
                _ => None,
            };
            let mut results = Vec::new();
            let mut cancelled = false;
            for (id, name, input, invalid) in &calls {
                if let Some(reason) =
                    blocked.or(cancelled.then_some("Not run: cancelled by the user."))
                {
                    results.push(tool_result(id, reason.into(), true));
                    continue;
                }
                let outcome = tokio::select! {
                    outcome = self.execute(id, name, input, invalid.as_deref()) => Some(outcome),
                    _ = cancel.notified() => None,
                };
                let (content, is_error) = outcome.unwrap_or_else(|| {
                    cancelled = true;
                    ("Cancelled by the user.".into(), true)
                });
                self.emit(AgentEvent::ToolFinished {
                    id: id.clone(),
                    ok: !is_error,
                    preview: preview(&content),
                });
                results.push(tool_result(id, content, is_error));
            }
            self.push(Message {
                role: Role::User,
                content: results,
            });

            if cancelled {
                self.emit(AgentEvent::Notice("Cancelled.".into()));
                return;
            }
            if let StopReason::Refusal { .. } = turn.stop {
                self.emit(AgentEvent::Error(refusal(&turn.stop)));
                return;
            }
        }
    }

    /// Run one tool call. Returns the result text and whether it is an error.
    async fn execute(
        &mut self,
        id: &str,
        name: &str,
        input: &Value,
        invalid_json: Option<&str>,
    ) -> (String, bool) {
        let started = |summary: String| AgentEvent::ToolStarted {
            id: id.to_owned(),
            summary,
        };
        if invalid_json.is_some() {
            self.emit(started(name.to_owned()));
            return (
                "INVALID_JSON: the tool input was not valid JSON (possibly cut off). Retry \
                 with smaller input."
                    .into(),
                true,
            );
        }
        let call = match cradle_tools::parse_call(name, input) {
            Ok(call) => call,
            Err(e) => {
                self.emit(started(name.to_owned()));
                return (e, true);
            }
        };
        self.emit(started(call.summary()));

        let mode = self.modes.get(&self.mode).expect("current mode exists");
        if call.access() == Access::Write && mode.access == ModeAccess::Read {
            return (
                format!(
                    "`{name}` is not available in `{}` mode, which is read-only. Finish \
                     planning and ask the user to switch to build mode.",
                    mode.name
                ),
                true,
            );
        }

        match self.run_call(call).await {
            Ok(text) => (text, false),
            Err(e) => (e.0, true),
        }
    }

    async fn run_call(&mut self, call: ToolCall) -> Result<String, ToolError> {
        use cradle_tools::fs as tfs;
        let reveal = |s: String| reveal_invisible(&s).into_owned();
        match call {
            ToolCall::ReadFile {
                path,
                offset,
                limit,
            } => tfs::read_file(&self.guard, &path, offset, limit).map(reveal),
            ToolCall::ListFiles { path, glob } => {
                tfs::list_files(&self.guard, path.as_deref(), glob.as_deref()).map(reveal)
            }
            ToolCall::Search {
                pattern,
                path,
                glob,
            } => tfs::search(&self.guard, &pattern, path.as_deref(), glob.as_deref()).map(reveal),
            ToolCall::GitDiff { path, staged } => {
                if let Some(path) = &path {
                    self.guard.resolve(path)?;
                }
                command::git_diff(self.guard.root(), path.as_deref(), staged).await
            }
            ToolCall::AskUser { question, options } => {
                let (reply, answer) = oneshot::channel();
                self.emit(AgentEvent::Question(QuestionRequest {
                    question,
                    options,
                    reply,
                }));
                answer
                    .await
                    .map(|a| format!("The user answered: {a}"))
                    .map_err(|_| ToolError::new("The question was dismissed."))
            }
            ToolCall::WriteFile { path, content } => {
                let write = tfs::prepare_write(&self.guard, &path, &content)?;
                self.apply_write(write).await
            }
            ToolCall::EditFile {
                path,
                old_string,
                new_string,
                replace_all,
            } => {
                let write =
                    tfs::prepare_edit(&self.guard, &path, &old_string, &new_string, replace_all)?;
                self.apply_write(write).await
            }
            ToolCall::RunCommand { name, reason } => self.run_command(&name, &reason).await,
        }
    }

    async fn apply_write(
        &mut self,
        write: cradle_tools::fs::PreparedWrite,
    ) -> Result<String, ToolError> {
        let rel = write.target.rel.to_string_lossy().into_owned();
        let note = match &write.class {
            WriteClass::Normal => None,
            WriteClass::Protected(reason) => Some(format!("Protected file: {reason}.")),
            WriteClass::AgentsDoc => Some("Project documentation in .agents/.".to_owned()),
        };
        if let Some(note) = note {
            let verb = if write.existed { "Change" } else { "Create" };
            let approval = self
                .ask(
                    format!("{verb} {rel}?"),
                    write.diff.clone(),
                    Some(note),
                    false,
                )
                .await;
            if approval == Approval::No {
                return Err(ToolError::new(
                    "The user declined this change. Ask them how they want to proceed.",
                ));
            }
        }

        if !self.originals.contains_key(&rel) {
            let before = if write.existed {
                std::fs::read_to_string(&write.target.abs).ok()
            } else {
                None
            };
            self.originals
                .insert(rel.clone(), (write.target.abs.clone(), before));
        }
        let summary = cradle_tools::fs::apply(&write)?;

        if let WriteClass::Protected(_) = write.class {
            self.protected_changed.insert(rel.clone());
            self.always_allowed.clear();
        }
        if rel == ".gitignore" {
            // Hidden files are derived from .gitignore; pick up the change.
            if let Ok(guard) = PathGuard::new(self.guard.root()) {
                self.guard = guard;
            }
        }
        self.emit(AgentEvent::Diff {
            title: summary.clone(),
            diff: write.diff,
        });
        Ok(summary)
    }

    async fn run_command(&mut self, name: &str, reason: &str) -> Result<String, ToolError> {
        let Some(line) = self.commands.get(name).cloned() else {
            let names: Vec<&str> = self.commands.keys().map(String::as_str).collect();
            return Err(ToolError::new(format!(
                "unknown command `{name}`; configured: {}",
                if names.is_empty() {
                    "none".into()
                } else {
                    names.join(", ")
                }
            )));
        };
        let argv = command::split(&line)?;
        let reason = reveal_invisible(reason).into_owned();

        let pre_approved = self.always_allowed.contains(name) && self.protected_changed.is_empty();
        if !pre_approved {
            let protected_note = (!self.protected_changed.is_empty()).then(|| {
                let files: Vec<&str> = self.protected_changed.iter().map(String::as_str).collect();
                format!(
                    "Build-affecting files changed this session: {}.",
                    files.join(", ")
                )
            });
            let mut note = protected_note.clone();
            let mut approved = false;

            if self.review == ReviewPolicy::Subagent {
                if protected_note.is_some() {
                    note = note.map(|n| format!("{n} The reviewer was skipped: you decide."));
                } else {
                    self.emit(AgentEvent::Notice(format!(
                        "Reviewer is checking `{name}`…"
                    )));
                    let changes = self.changed_files();
                    let review =
                        review::review_command(&self.backend, name, &line, &reason, &changes).await;
                    self.emit(AgentEvent::ReviewResult {
                        command: name.to_owned(),
                        verdict: review.verdict,
                        reason: review.reason.clone(),
                    });
                    if review.verdict == Verdict::Safe {
                        approved = true;
                    } else {
                        let verdict = match review.verdict {
                            Verdict::Unsafe => "unsafe",
                            _ => "unsure",
                        };
                        note = Some(format!("Reviewer says {verdict}: {}", review.reason));
                    }
                }
            }

            if !approved {
                let body = format!("$ {line}\n\nReason given by the agent:\n{reason}");
                match self.ask(format!("Run `{name}`?"), body, note, true).await {
                    Approval::Yes => {
                        self.protected_changed.clear();
                    }
                    Approval::Always => {
                        self.protected_changed.clear();
                        self.always_allowed.insert(name.to_owned());
                    }
                    Approval::No => {
                        return Err(ToolError::new(format!(
                            "The user declined to run `{name}`."
                        )));
                    }
                }
            }
        }

        command::run(&argv, self.guard.root(), command::DEFAULT_TIMEOUT).await
    }

    /// Diffs of every file changed this session, for the reviewer.
    fn changed_files(&self) -> Vec<ChangedFile> {
        self.originals
            .iter()
            .filter_map(|(rel, (abs, before))| {
                let now = std::fs::read_to_string(abs).unwrap_or_default();
                let diff = cradle_tools::fs::unified_diff(rel, before.as_deref(), &now);
                (!diff.is_empty()).then(|| ChangedFile {
                    path: rel.clone(),
                    diff,
                })
            })
            .collect()
    }

    async fn ask(
        &self,
        title: String,
        body: String,
        note: Option<String>,
        allow_always: bool,
    ) -> Approval {
        let (reply, answer) = oneshot::channel();
        self.emit(AgentEvent::Approval(ApprovalRequest {
            title,
            body,
            note,
            allow_always,
            reply,
        }));
        answer.await.unwrap_or(Approval::No)
    }

    fn push(&mut self, message: Message) {
        if let Some(session) = &mut self.session
            && let Err(e) = session.append(&message)
        {
            let _ = self.events.send(AgentEvent::Error(format!(
                "cannot write session log {}: {e}",
                session.path().display()
            )));
            self.session = None;
        }
        self.history.push(message);
    }

    fn emit(&self, event: AgentEvent) {
        let _ = self.events.send(event);
    }

    fn emit_mode(&self) {
        let mode = self.modes.get(&self.mode).expect("current mode exists");
        self.emit(AgentEvent::ModeChanged {
            name: mode.name.clone(),
            read_only: mode.access == ModeAccess::Read,
        });
    }
}

fn tool_result(id: &str, content: String, is_error: bool) -> Block {
    Block::ToolResult {
        tool_use_id: id.to_owned(),
        content,
        is_error,
    }
}

fn refusal(stop: &StopReason) -> String {
    match stop {
        StopReason::Refusal {
            category,
            explanation,
        } => {
            let mut text = "The model declined to continue".to_owned();
            if let Some(c) = category {
                text.push_str(&format!(" ({c})"));
            }
            if let Some(e) = explanation {
                text.push_str(&format!(": {e}"));
            }
            text
        }
        _ => String::new(),
    }
}

fn preview(content: &str) -> String {
    let lines: Vec<&str> = content.lines().take(PREVIEW_LINES).collect();
    let more = content.lines().count().saturating_sub(PREVIEW_LINES);
    let mut text = lines.join("\n");
    if more > 0 {
        text.push_str(&format!("\n… {more} more lines"));
    }
    text
}

/// A resumed transcript may end mid-turn. Answer any tool calls left without results so
/// the next request is valid.
fn close_dangling_tool_calls(history: &mut Vec<Message>) {
    let Some(last) = history.last() else { return };
    if last.role != Role::Assistant {
        return;
    }
    let results: Vec<Block> = last
        .tool_uses()
        .map(|c| tool_result(c.id, "Not run: the session was interrupted.".into(), true))
        .collect();
    if !results.is_empty() {
        history.push(Message {
            role: Role::User,
            content: results,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn closes_dangling_tool_calls() {
        let mut history = vec![
            Message::user("hi"),
            Message {
                role: Role::Assistant,
                content: vec![Block::ToolUse {
                    id: "t1".into(),
                    name: "read_file".into(),
                    input: json!({"path": "a"}),
                    invalid_json: None,
                }],
            },
        ];
        close_dangling_tool_calls(&mut history);
        assert_eq!(history.len(), 3);
        assert!(matches!(
            &history[2].content[0],
            Block::ToolResult { tool_use_id, is_error: true, .. } if tool_use_id == "t1"
        ));
        close_dangling_tool_calls(&mut history);
        assert_eq!(history.len(), 3, "nothing to close twice");
    }

    #[test]
    fn previews_are_short() {
        assert_eq!(preview("a\nb"), "a\nb");
        assert_eq!(preview("1\n2\n3\n4\n5\n6"), "1\n2\n3\n4\n… 2 more lines");
    }
}

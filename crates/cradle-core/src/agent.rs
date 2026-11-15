//! The agent loop. Runs on its own task and talks to the UI over channels.
//!
//! Enforcement lives here, not in the prompt:
//! - Read-only modes refuse write tools whatever the model asks for.
//! - Writes to protected files and `.agents/` docs need the user's approval (with a diff).
//! - Every `run_command` is approved by the user, or by the reviewer subagent when that
//!   policy is on. The reviewer is skipped (the user decides) once any protected file has
//!   changed this session.
//! - A mode's own commands (`[<name>_mode.commands]`) are reviewed on the first message in
//!   that mode, under the reviewer policy. Safe ones run without asking; the rest are
//!   blocked for the session, by command line. Read-only modes may run only these.
//! - Project config is reloaded before every message. A changed config applies only once
//!   the user trusts it, after seeing the diff.
//! - A project preference that differs from the user's applies only once the user picks
//!   it. With `auto_verify`, `build`/`test` run after edits, through the usual approvals.
//! - History is append-only, which the API's thinking blocks require.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cradle_backends::{
    AgentTurn, Backend, Block, McpEndpoint, Message, ModelRequest, Role, StopReason, StreamEvent,
    ToolSpec, Usage,
};
use cradle_config::soul::{Soul, SoulKey, SoulPatch};
use cradle_config::{Detection, ModeAccess, ProjectConfig, SourceKind};
use cradle_mcp::{McpServer, McpTool, ToolReply, ToolRequest};
use cradle_tools::hygiene::reveal_invisible;
use cradle_tools::{
    Access, PathGuard, Question, QuestionKind, ToolActivity, ToolCall, ToolDefinition, ToolError,
    VarStore, WriteClass, command, vars,
};
use serde_json::Value;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::modes::{ClashChoice, FileEdits, ModeSet, SCAFFOLD_MODE};
use crate::prompt;
use crate::review::{self, ChangedFile, Verdict};
use crate::session::SessionLog;
use crate::soul::{self as prefs, SoulChoice};
use crate::trust::{self, TrustStore};

const MAX_TOKENS: u32 = 64_000;
const PREVIEW_LINES: usize = 4;
/// What `auto_verify` runs, in order, if the project has them.
const VERIFY_COMMANDS: [&str; 2] = ["build", "test"];

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
        /// What the call does, for grouping. `None` if the input did not parse.
        activity: Option<ToolActivity>,
    },
    ToolFinished {
        id: String,
        ok: bool,
        preview: String,
        /// `item: error` for each part that failed in a call that otherwise worked
        /// (e.g. one file of a `read_files`).
        failed: Vec<String>,
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
    /// The available modes changed (the project config was reloaded): each name and
    /// whether it is read-only.
    ModesChanged(Vec<(String, bool)>),
    ModelChanged {
        provider: String,
        model: String,
    },
    ReviewChanged(ReviewPolicy),
    Usage(Usage),
    /// Every file changed this session, after each change.
    FilesChanged(Vec<FileStat>),
    /// A saved conversation replaced the current one.
    SessionLoaded(Vec<Message>),
    Notice(String),
    Error(String),
    TurnFinished,
}

/// Lines added and removed in one file since the session started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStat {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

#[derive(Debug)]
pub struct ApprovalRequest {
    pub title: String,
    pub body: String,
    pub note: Option<String>,
    /// The label for "allow this for the rest of the session", if offered.
    pub always: Option<String>,
    pub reply: oneshot::Sender<Approval>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Approval {
    Yes,
    Always,
    No,
    /// Not this; the user says what to do instead.
    Change(String),
}

/// One or more questions for the user, answered in order. Dropping `reply` dismisses them.
#[derive(Debug)]
pub struct QuestionRequest {
    pub questions: Vec<Question>,
    /// One answer per question, in the same order.
    pub reply: oneshot::Sender<Vec<Answer>>,
}

/// The user's answer to one [`Question`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// One of the options.
    Choice(String),
    /// Options picked on a multi-choice question, plus an own answer if typed.
    Choices {
        picked: Vec<String>,
        other: Option<String>,
    },
    /// Typed text: the answer to a text question, or an own answer to a choice question.
    Text(String),
}

/// The tool result for answered questions. `questions` and `answers` are in the same order.
fn format_answers(questions: &[Question], answers: &[Answer]) -> String {
    let answer = |q: &Question, a: &Answer| match a {
        Answer::Choice(c) => c.clone(),
        Answer::Choices { picked, other } => {
            let mut parts = Vec::new();
            if !picked.is_empty() {
                parts.push(picked.join(", "));
            }
            if let Some(other) = other {
                parts.push(format!("other: \"{other}\""));
            }
            parts.join("; ")
        }
        Answer::Text(t) if q.kind == QuestionKind::Text => t.clone(),
        Answer::Text(t) => format!("(own answer) {t}"),
    };
    match (questions, answers) {
        ([q], [a]) => format!("The user answered: {}", answer(q, a)),
        _ => {
            let mut text = "The user answered:".to_owned();
            for (n, (q, a)) in questions.iter().zip(answers).enumerate() {
                text.push_str(&format!("\n{}. {}\n   {}", n + 1, q.question, answer(q, a)));
            }
            text
        }
    }
}

#[derive(Debug)]
pub enum AgentCommand {
    Send(String),
    SetMode(String),
    CycleMode,
    /// Cycle modes in reverse.
    CycleModeBack,
    SetReview(ReviewPolicy),
    ShowDiff,
    Clear,
    /// Continue a saved transcript instead of the current conversation.
    LoadSession(PathBuf),
    /// Switch model or provider. The conversation continues on the new model.
    SetBackend(Box<Backend>),
    /// The user changed their preferences. They apply from the next message.
    SetSoul(Soul),
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
    /// `trust.toml`, for approving a changed project config mid-session. `None`: approvals
    /// last only for this session.
    pub trust_store: Option<PathBuf>,
    /// The user's global modes (`~/.config/cradle/modes/`), for re-resolving modes.
    pub global_modes: Option<PathBuf>,
    /// The user's own preferences (`~/.config/cradle/soul.toml`, or the defaults).
    pub soul: Soul,
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
    /// The applied project config (`None`: absent or not trusted).
    project: Option<ProjectConfig>,
    /// The snapshot and hash of `project`, to notice and show changes.
    applied: Option<(String, String)>,
    /// A changed config the user declined this session, by hash: not asked about again.
    declined_hash: Option<String>,
    /// The last config load error shown, so it is not repeated every message.
    config_error: Option<String>,
    trust_store: Option<PathBuf>,
    global_modes: Option<PathBuf>,
    /// Command lines the user allowed for the rest of the session.
    always_allowed: HashSet<String>,
    /// Mode command lines the mode reviewer judged safe this session.
    mode_safe: HashSet<String>,
    /// Mode command lines the mode reviewer did not pass, with its reason. Blocked for the
    /// session wherever they are configured.
    blocked: BTreeMap<String, String>,
    /// Each mode's commands as last reviewed, so a mode is reviewed once per session (and
    /// again only if its commands change).
    reviewed_modes: HashMap<String, BTreeMap<String, String>>,
    /// Content of each changed file before its first change this session (`None`: new).
    originals: BTreeMap<String, (PathBuf, Option<String>)>,
    /// Protected files changed since the user last approved a command.
    protected_changed: BTreeSet<String>,
    /// For CLI backends: Cradle's MCP tool server and the calls it receives.
    mcp: Option<(McpServer, mpsc::UnboundedReceiver<ToolRequest>)>,
    /// For CLI backends: the CLI's own session id, to continue the conversation.
    cli_session: Option<String>,
    /// The user allowed every file edit for the rest of the session.
    edits_allowed: bool,
    /// Parts that failed in the tool call that just ran, for its `ToolFinished`.
    failed_parts: Vec<String>,
    /// Tool results saved with `save_as` in this conversation.
    vars: VarStore,
    /// The user's own preferences.
    user_soul: Soul,
    /// The preferences that apply: the user's, with project values they picked.
    soul: Soul,
    /// The preferences the model was last told about in this conversation.
    announced_soul: Option<Soul>,
    /// Files changed since the user's last message, for `auto_verify`.
    edited: bool,
    /// `auto_verify` already ran for the user's last message.
    verified: bool,
    /// Automatic check runs so far, for tool card ids.
    verify_runs: usize,
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
        let (tools, tool_specs) = tool_set(setup.project.as_ref());
        let system = prompt::system_prompt(setup.guard.root(), setup.project.as_ref());

        let applied = setup.project.as_ref().and_then(|p| {
            let snapshot = trust::snapshot(p).ok()?;
            Some((trust::snapshot_hash(&snapshot), snapshot))
        });
        // Without a trusted project, any config on disk was declined at startup (or there
        // is none). Don't ask about it again until it changes.
        let store = setup
            .trust_store
            .as_ref()
            .and_then(|p| TrustStore::load(p.clone()).ok());
        let declined_hash = if setup.project.is_none() {
            load_project(setup.guard.root(), store.as_ref(), None)
                .ok()
                .flatten()
                .and_then(|p| trust::fingerprint(&p).ok())
        } else {
            None
        };
        // Undecided project preferences are asked about with the first message.
        let (soul, _) = resolve_soul(
            &setup.soul,
            setup.project.as_ref(),
            setup.guard.root(),
            store.as_ref(),
        );

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
            project: setup.project,
            applied,
            declined_hash,
            config_error: None,
            trust_store: setup.trust_store,
            global_modes: setup.global_modes,
            always_allowed: HashSet::new(),
            mode_safe: HashSet::new(),
            blocked: BTreeMap::new(),
            reviewed_modes: HashMap::new(),
            originals: BTreeMap::new(),
            protected_changed: BTreeSet::new(),
            mcp: None,
            cli_session: None,
            edits_allowed: false,
            failed_parts: Vec::new(),
            vars: VarStore::default(),
            user_soul: setup.soul,
            soul,
            announced_soul: None,
            edited: false,
            verified: false,
            verify_runs: 0,
            events,
            cancel,
        }
    }

    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<AgentCommand>) {
        self.emit_mode();
        self.emit(AgentEvent::ReviewChanged(self.review));
        if self.backend.is_cli() {
            self.start_mcp().await;
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
                AgentCommand::CycleModeBack => {
                    self.mode = self.modes.prev_before(&self.mode).name.clone();
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
                    if backend.provider() != self.backend.provider() {
                        // A CLI's session id means nothing to another provider.
                        self.cli_session = None;
                        self.announced_mode = None;
                        self.announced_soul = None;
                        if backend.is_cli() && !self.history.is_empty() {
                            self.emit(AgentEvent::Notice(format!(
                                "{} starts a fresh session: earlier messages are not sent.",
                                backend.provider()
                            )));
                        }
                    }
                    if backend.is_cli() && self.mcp.is_none() {
                        self.start_mcp().await;
                    }
                    if !backend.supports_tools() {
                        self.emit(AgentEvent::Notice(format!(
                            "{} does not support tools: chat only, no file access.",
                            backend.provider()
                        )));
                    }
                    self.backend = Arc::new(*backend);
                    self.emit(AgentEvent::ModelChanged {
                        provider: self.backend.provider().to_owned(),
                        model: self.backend.model().to_owned(),
                    });
                }
                AgentCommand::SetSoul(soul) => {
                    self.user_soul = soul;
                    let store = self.load_store();
                    self.soul = resolve_soul(
                        &self.user_soul,
                        self.project.as_ref(),
                        self.guard.root(),
                        store.as_ref(),
                    )
                    .0;
                }
                AgentCommand::Clear => {
                    self.history.clear();
                    self.vars.clear();
                    self.announced_mode = None;
                    self.announced_soul = None;
                    self.cli_session = None;
                    self.reset_mode_reviews();
                    if let (Some(dir), Some(_)) = (&self.data_dir, &self.session) {
                        self.session = SessionLog::create(dir, self.guard.root()).ok();
                    }
                    self.emit(AgentEvent::Notice("Started a new conversation.".into()));
                }
                AgentCommand::LoadSession(path) => match crate::session::read(&path) {
                    Ok(mut history) => {
                        close_dangling_tool_calls(&mut history);
                        match SessionLog::open(path.clone()) {
                            Ok(log) => self.session = Some(log),
                            Err(e) => self.emit(AgentEvent::Error(format!(
                                "cannot append to {}: {e}",
                                path.display()
                            ))),
                        }
                        self.history = history;
                        self.vars.clear();
                        self.announced_mode = None;
                        self.announced_soul = None;
                        self.cli_session = None;
                        self.reset_mode_reviews();
                        self.emit(AgentEvent::SessionLoaded(self.history.clone()));
                        if self.backend.is_cli() {
                            self.emit(AgentEvent::Notice(format!(
                                "{} starts a fresh session: the resumed messages are shown but \
                                 not sent.",
                                self.backend.provider()
                            )));
                        }
                    }
                    Err(e) => self.emit(AgentEvent::Error(format!(
                        "cannot read {}: {e}",
                        path.display()
                    ))),
                },
            }
        }
    }

    /// Start Cradle's MCP tool server, which CLI backends call for every tool.
    async fn start_mcp(&mut self) {
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
    }

    /// Reload the project config from disk. An unchanged config does nothing. A changed
    /// one applies at once if the user already trusts it; otherwise the user sees the diff
    /// and decides. Returns `false` if the user cancelled the turn while deciding.
    async fn reload_config(&mut self) -> bool {
        let root = self.guard.root().to_path_buf();
        let mut store = self
            .trust_store
            .as_ref()
            .and_then(|p| TrustStore::load(p.clone()).ok());

        let current_source = self.project.as_ref().map(|p| p.source);
        let project = match load_project(&root, store.as_ref(), current_source) {
            Ok(project) => {
                self.config_error = None;
                project
            }
            Err(e) => {
                if self.config_error.as_deref() != Some(e.as_str()) {
                    self.emit(AgentEvent::Error(format!(
                        "The project config has an error; keeping the previous config: {e}"
                    )));
                    self.config_error = Some(e);
                }
                return true;
            }
        };

        let Some(project) = project else {
            // Removing the config only takes permissions away: apply without asking.
            if self.project.is_some() {
                self.apply_config(None, None, &mut store).await;
                self.emit(AgentEvent::Notice(
                    "The project config was removed: running without project modes and commands."
                        .into(),
                ));
            }
            return true;
        };
        let snapshot = match trust::snapshot(&project) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                self.emit(AgentEvent::Error(format!(
                    "cannot read the project config: {e}"
                )));
                return true;
            }
        };
        let hash = trust::snapshot_hash(&snapshot);

        if self.applied.as_ref().is_some_and(|(h, _)| *h == hash) {
            // Nothing trusted changed. Docs (product.md, architecture.md, ...) may have.
            if self.project.as_ref() != Some(&project) {
                self.apply_config(Some(project), Some((hash, snapshot)), &mut store)
                    .await;
            }
            return true;
        }
        if store.as_ref().is_some_and(|s| s.is_trusted(&root, &hash)) {
            self.apply_config(Some(project), Some((hash, snapshot)), &mut store)
                .await;
            self.emit(AgentEvent::Notice(
                "The project config changed to a version you already trust; applied it.".into(),
            ));
            return true;
        }
        if self.declined_hash.as_deref() == Some(hash.as_str()) {
            return true;
        }

        // Diff against what the user last approved: the stored snapshot, else the one
        // running now.
        let approved = store
            .as_ref()
            .and_then(|s| s.approved_snapshot(&root))
            .or_else(|| self.applied.as_ref().map(|(_, s)| s.clone()));
        let title = if approved.is_some() {
            "The project config changed. Trust and apply it?"
        } else {
            "Trust this project config and apply it?"
        };
        let diff = trust::config_diff(approved.as_deref(), &snapshot);
        let note = Some(
            "Approve to apply it now. Decline to keep the current config for this session \
             (Cradle asks again if it changes)."
                .to_owned(),
        );
        let cancel = self.cancel.clone();
        let answer = tokio::select! {
            answer = self.ask(title.to_owned(), diff, note, None) => answer,
            _ = cancel.notified() => return false,
        };
        match answer {
            Approval::Yes | Approval::Always => {
                if let Some(store) = store.as_mut()
                    && let Err(e) = store.trust(&root, &snapshot)
                {
                    self.emit(AgentEvent::Error(format!(
                        "cannot save the approval (it applies to this session only): {e}"
                    )));
                }
                self.apply_config(Some(project), Some((hash, snapshot)), &mut store)
                    .await;
                self.emit(AgentEvent::Notice("Applied the new project config.".into()));
            }
            Approval::No | Approval::Change(_) => {
                self.declined_hash = Some(hash);
                self.emit(AgentEvent::Notice(
                    "Kept the current project config.".into(),
                ));
            }
        }
        true
    }

    /// Switch to a new (trusted) project config: commands, tools, system prompt and modes.
    async fn apply_config(
        &mut self,
        project: Option<ProjectConfig>,
        applied: Option<(String, String)>,
        store: &mut Option<TrustStore>,
    ) {
        let choice = self.clash_choice(project.as_ref(), store).await;
        let modes = match ModeSet::resolve(
            project.as_ref().map(|p| &p.modes),
            self.global_modes.as_deref(),
            choice,
        ) {
            Ok(modes) => modes,
            Err(e) => {
                self.emit(AgentEvent::Error(format!(
                    "cannot read mode files; keeping the previous config: {e}"
                )));
                return;
            }
        };

        let (tools, tool_specs) = tool_set(project.as_ref());
        let tools_changed =
            self.tools.len() != tools.len()
                || self.tools.iter().zip(&tools).any(|(a, b)| {
                    a.description != b.description || a.input_schema != b.input_schema
                });
        self.tools = tools;
        self.tool_specs = tool_specs;
        self.vars.clear();
        self.system = prompt::system_prompt(self.guard.root(), project.as_ref());
        self.commands = project
            .as_ref()
            .map(|p| p.commands.clone())
            .unwrap_or_default();
        self.modes = modes;
        if self.modes.get(&self.mode).is_none() {
            self.emit(AgentEvent::Notice(format!(
                "Mode `{}` no longer exists; switched to build.",
                self.mode
            )));
            self.mode = "build".to_owned();
        }
        // The mode's instructions or commands may have changed: tell the model again.
        self.announced_mode = None;
        self.project = project;
        self.applied = applied;
        self.declined_hash = None;
        // New project preferences are asked about before the message goes out.
        self.soul = resolve_soul(
            &self.user_soul,
            self.project.as_ref(),
            self.guard.root(),
            store.as_ref(),
        )
        .0;

        let modes = self
            .modes
            .iter()
            .map(|m| (m.name.clone(), m.access == ModeAccess::Read))
            .collect();
        self.emit(AgentEvent::ModesChanged(modes));
        self.emit_mode();

        // A CLI backend got Cradle's tools when its tool server started.
        if tools_changed && self.mcp.is_some() {
            self.mcp = None;
            self.start_mcp().await;
            if self.cli_session.take().is_some() {
                self.emit(AgentEvent::Notice(format!(
                    "{} starts a fresh session: Cradle's tools changed with the config.",
                    self.backend.provider()
                )));
            }
        }
    }

    /// How to combine project and global modes: the project's `clash_handler`, else the
    /// stored choice, else ask the user (and store the answer).
    async fn clash_choice(
        &self,
        project: Option<&ProjectConfig>,
        store: &mut Option<TrustStore>,
    ) -> ClashChoice {
        let modes = project.map(|p| &p.modes);
        if let Some(choice) = modes.and_then(|m| ClashChoice::from_handler(m.clash_handler)) {
            return choice;
        }
        let clashes = ModeSet::clashes(modes, self.global_modes.as_deref());
        if clashes.is_empty() {
            return ClashChoice::Override;
        }
        let root = self.guard.root();
        if let Some(choice) = store.as_ref().and_then(|s| s.clash_choice(root)) {
            return choice;
        }
        let names: Vec<&str> = clashes.iter().map(|c| c.mode.as_str()).collect();
        let (override_label, combine_label) = (
            "Use the project's version (override)",
            "Use both: global first, then the project's (combine)",
        );
        let (reply, answer) = oneshot::channel();
        self.emit(AgentEvent::Question(QuestionRequest {
            questions: vec![Question::single(
                format!(
                    "Both your global config and this project define these modes: {}. Which \
                     should apply?",
                    names.join(", ")
                ),
                vec![override_label.to_owned(), combine_label.to_owned()],
            )],
            reply,
        }));
        let choice = match answer.await.as_deref() {
            Ok([Answer::Choice(a)]) if a == combine_label => ClashChoice::Combine,
            _ => ClashChoice::Override,
        };
        if let Some(store) = store.as_mut() {
            let _ = store.set_clash_choice(root, choice);
        }
        choice
    }

    fn load_store(&self) -> Option<TrustStore> {
        self.trust_store
            .as_ref()
            .and_then(|p| TrustStore::load(p.clone()).ok())
    }

    /// Ask about project preferences that differ from the user's own and haven't been
    /// decided (or whose project value changed). The answers are stored per project.
    /// Dismissed: the user's own values apply, and Cradle asks again next message.
    /// Returns `false` if the user cancelled.
    async fn settle_soul(&mut self) -> bool {
        let mut store = self.load_store();
        let root = self.guard.root().to_path_buf();
        let (soul, open) = resolve_soul(
            &self.user_soul,
            self.project.as_ref(),
            &root,
            store.as_ref(),
        );
        self.soul = soul;
        if open.is_empty() {
            return true;
        }
        let patch = self
            .project
            .as_ref()
            .map(|p| p.soul.clone())
            .unwrap_or_default();
        let questions = open
            .iter()
            .map(|key| prefs::question(*key, &self.user_soul, &patch))
            .collect();
        let (reply, answer) = oneshot::channel();
        self.emit(AgentEvent::Question(QuestionRequest { questions, reply }));
        let cancel = self.cancel.clone();
        let answers = tokio::select! {
            answers = answer => answers,
            _ = cancel.notified() => return false,
        };
        let Ok(answers) = answers else {
            self.emit(AgentEvent::Notice(
                "Using your own preferences for now; Cradle asks about the project's again with \
                 your next message."
                    .into(),
            ));
            return true;
        };
        let theirs = self.user_soul.with(&patch);
        for (key, answer) in open.iter().zip(&answers) {
            let [use_project, _] = prefs::options(*key, &self.user_soul, &patch);
            let choice = match answer {
                Answer::Choice(c) if *c == use_project => SoulChoice::Project,
                _ => SoulChoice::Mine,
            };
            if choice == SoulChoice::Project {
                self.soul.take(*key, &theirs);
            }
            if let Some(store) = store.as_mut()
                && let Err(e) = store.set_soul_choice(&root, *key, &theirs.value(*key), choice)
            {
                self.emit(AgentEvent::Error(format!(
                    "cannot save the choice (it applies to this session only): {e}"
                )));
            }
        }
        true
    }

    /// Tell the model the preferences if it doesn't know the current ones. Returns the
    /// message.
    fn announce_soul(&mut self) -> Option<String> {
        if self.announced_soul == Some(self.soul) {
            return None;
        }
        let message = prompt::soul_message(&self.soul);
        self.push(Message::system(message.clone()));
        self.announced_soul = Some(self.soul);
        Some(message)
    }

    /// With `auto_verify` on, after a reply that changed files: run the project's `build`
    /// and `test` commands (the ones that exist), through the usual approvals. Once per user
    /// message. Returns a report for the model if a check failed; a command that didn't
    /// run (declined, blocked) just ends the check.
    async fn auto_verify(&mut self) -> Option<String> {
        if !self.soul.auto_verify || !self.edited || self.verified {
            return None;
        }
        let mode = self.modes.get(&self.mode)?;
        if mode.access != ModeAccess::Full {
            return None;
        }
        let names: Vec<&str> = VERIFY_COMMANDS
            .into_iter()
            .filter(|n| self.commands.contains_key(*n) || mode.commands.contains_key(*n))
            .collect();
        self.verified = true;
        if names.is_empty() {
            self.emit(AgentEvent::Notice(
                "auto_verify is on, but this project has no `build` or `test` command.".into(),
            ));
            return None;
        }

        const REASON: &str =
            "Automatic check after file changes (auto_verify in your preferences).";
        let cancel = self.cancel.clone();
        let mut passed = Vec::new();
        for name in names {
            self.verify_runs += 1;
            let id = format!("verify-{}", self.verify_runs);
            let call = ToolCall::RunCommand {
                name: name.to_owned(),
                reason: REASON.to_owned(),
            };
            self.emit(AgentEvent::ToolStarted {
                id: id.clone(),
                summary: format!("{} (auto_verify)", call.summary()),
                activity: Some(call.activity()),
            });
            let outcome = tokio::select! {
                outcome = self.run_command(name, REASON) => outcome,
                _ = cancel.notified() => Err(ToolError::new("Cancelled.")),
            };
            // A failing command still reports (non-zero exit); `Err` means it didn't run.
            let (text, ran) = match outcome {
                Ok(report) => (report, true),
                Err(e) => (e.0, false),
            };
            let ok = ran && text.starts_with("exit code 0");
            self.emit(AgentEvent::ToolFinished {
                id,
                ok,
                preview: preview(&text),
                failed: Vec::new(),
            });
            if !ran {
                self.emit(AgentEvent::Notice(format!(
                    "auto_verify stopped: `{name}` did not run."
                )));
                return None;
            }
            if !ok {
                return Some(format!(
                    "Automatic check after your changes (auto_verify): `{name}` failed.\n\n\
                     {text}\n\nFix the cause, then summarise what you changed. The check does \
                     not run again for this message."
                ));
            }
            passed.push(format!("`{name}`"));
        }
        self.emit(AgentEvent::Notice(format!(
            "auto_verify: {} passed.",
            passed.join(" and ")
        )));
        None
    }

    /// On the first message in a mode, have the reviewer check the mode's own commands.
    /// Safe ones may then run without approval; the rest are blocked for the session.
    /// Only under the reviewer policy. Returns `false` if the user cancelled.
    async fn review_mode(&mut self) -> bool {
        if self.review != ReviewPolicy::Subagent {
            return true;
        }
        let mode = self.modes.get(&self.mode).expect("current mode exists");
        if mode.commands.is_empty() || self.reviewed_modes.get(&mode.name) == Some(&mode.commands) {
            return true;
        }
        let name = mode.name.clone();
        let all = mode.commands.clone();
        // Lines already judged this session (in another mode, or before a reload) keep
        // their verdict.
        let pending: BTreeMap<String, String> = all
            .iter()
            .filter(|(_, line)| {
                !self.mode_safe.contains(*line) && !self.blocked.contains_key(*line)
            })
            .map(|(n, l)| (n.clone(), l.clone()))
            .collect();

        if !pending.is_empty() {
            self.emit(AgentEvent::Notice(format!(
                "Reviewer is checking the `{name}` mode's commands…"
            )));
            let backend = self.backend.clone();
            let cancel = self.cancel.clone();
            let reviews = tokio::select! {
                reviews = review::review_mode_commands(&backend, &name, &pending) => reviews,
                _ = cancel.notified() => return false,
            };
            for (command, review) in reviews {
                let line = pending[&command].clone();
                self.emit(AgentEvent::ReviewResult {
                    command: format!("{command} ({name} mode)"),
                    verdict: review.verdict,
                    reason: review.reason.clone(),
                });
                if review.verdict == Verdict::Safe {
                    self.mode_safe.insert(line);
                } else {
                    self.emit(AgentEvent::Notice(format!(
                        "Blocked `{command}` (`{line}`) for this session: the reviewer did not \
                         judge it safe."
                    )));
                    let reason = if review.reason.is_empty() {
                        "no reason given".to_owned()
                    } else {
                        review.reason
                    };
                    self.blocked.insert(line, reason);
                }
            }
        }
        self.reviewed_modes.insert(name, all);
        true
    }

    /// A new session: mode commands are reviewed again and blocks are lifted.
    fn reset_mode_reviews(&mut self) {
        self.mode_safe.clear();
        self.blocked.clear();
        self.reviewed_modes.clear();
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
    /// `from_harness`: `text` is a harness message (an `auto_verify` report), not the user's.
    async fn run_cli_turn(&mut self, text: String, from_harness: bool) {
        let mut notices = Vec::new();
        if from_harness {
            self.push(Message::system(text.clone()));
            notices.push(text.clone());
        } else {
            self.push(Message::user(text.clone()));
        }
        notices.extend(self.announce_mode());
        notices.extend(self.announce_soul());
        let mut prompt = String::new();
        if !notices.is_empty() {
            prompt = format!(
                "<harness_notice>\n{}\n</harness_notice>\n\n",
                notices.join("\n\n")
            );
        }
        if !from_harness {
            prompt.push_str(&text);
        }

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
                    let failed = std::mem::take(&mut self.failed_parts);
                    let Some((content, is_error)) = result else {
                        self.emit(AgentEvent::ToolFinished {
                            id,
                            ok: false,
                            preview: "Cancelled.".into(),
                            failed,
                        });
                        break None;
                    };
                    self.emit(AgentEvent::ToolFinished {
                        id,
                        ok: !is_error,
                        preview: preview(&content),
                        failed,
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
        if !self.reload_config().await || !self.review_mode().await || !self.settle_soul().await {
            self.emit(AgentEvent::Notice("Cancelled.".into()));
            return;
        }
        self.edited = false;
        self.verified = false;
        if self.backend.is_cli() {
            self.run_cli_turn(text, false).await;
            if let Some(report) = self.auto_verify().await {
                self.run_cli_turn(report, true).await;
            }
            return;
        }
        self.push(Message::user(text));
        self.announce_mode();
        self.announce_soul();

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
                    _ => {
                        if let Some(report) = self.auto_verify().await {
                            self.push(Message::system(report));
                            continue;
                        }
                    }
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
                let failed = std::mem::take(&mut self.failed_parts);
                self.emit(AgentEvent::ToolFinished {
                    id: id.clone(),
                    ok: !is_error,
                    preview: preview(&content),
                    failed,
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
        self.failed_parts.clear();
        let started = |summary: String, activity: Option<ToolActivity>| AgentEvent::ToolStarted {
            id: id.to_owned(),
            summary,
            activity,
        };
        if invalid_json.is_some() {
            self.emit(started(name.to_owned(), None));
            return (
                "INVALID_JSON: the tool input was not valid JSON (possibly cut off). Retry \
                 with smaller input."
                    .into(),
                true,
            );
        }
        let parsed = vars::take_save_as(input)
            .and_then(|(input, save_as)| Ok((cradle_tools::parse_call(name, &input)?, save_as)));
        let (call, save_as) = match parsed {
            Ok(parsed) => parsed,
            Err(e) => {
                self.emit(started(name.to_owned(), None));
                return (e, true);
            }
        };
        let source = call.summary();
        let shown = match &save_as {
            Some(var) => format!("{source} → ${var}"),
            None => source.clone(),
        };
        self.emit(started(shown, Some(call.activity())));
        let shows_variable = call.shows_variable();

        let mode = self.modes.get(&self.mode).expect("current mode exists");
        let file_write = matches!(call, ToolCall::WriteFile { .. } | ToolCall::EditFile { .. });
        if file_write && mode.access == ModeAccess::Full && mode.file_edits == FileEdits::Deny {
            return (
                format!(
                    "File edits are turned off in `{}` mode. Describe the change instead, or \
                     ask the user to switch modes.",
                    mode.name
                ),
                true,
            );
        }
        // Read-only modes may run their own commands, and nothing else that writes.
        let own_command = matches!(
            &call,
            ToolCall::RunCommand { name, .. } if mode.commands.contains_key(name)
        );
        if call.access() == Access::Write && mode.access == ModeAccess::Read && !own_command {
            let also = if mode.commands.is_empty() {
                String::new()
            } else {
                let names: Vec<&str> = mode.commands.keys().map(String::as_str).collect();
                format!(" Its own commands are available: {}.", names.join(", "))
            };
            return (
                format!(
                    "`{name}` is not available in `{}` mode, which is read-only.{also} Finish \
                     planning and ask the user to switch to build mode.",
                    mode.name
                ),
                true,
            );
        }

        match (self.run_call(call).await, save_as) {
            (Ok(text), Some(var)) => match self.vars.save(&var, text, source) {
                Ok(summary) => (summary, false),
                Err(e) => (e.0, true),
            },
            (Ok(text), None) if shows_variable => (vars::cap_output(&text), false),
            (Ok(text), None) => (text, false),
            (Err(e), _) => (e.0, true),
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
            ToolCall::ReadFiles { files } => {
                let read = tfs::read_files(&self.guard, &files)
                    .map_err(|e| ToolError::new(reveal(e.0)))?;
                self.failed_parts = read.failed.into_iter().map(reveal).collect();
                Ok(reveal(read.output))
            }
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
            ToolCall::AskUser { questions } => {
                let (reply, answers) = oneshot::channel();
                self.emit(AgentEvent::Question(QuestionRequest {
                    questions: questions.clone(),
                    reply,
                }));
                let dismissed = || {
                    ToolError::new(if questions.len() == 1 {
                        "The question was dismissed."
                    } else {
                        "The questions were dismissed."
                    })
                };
                match answers.await {
                    Ok(answers) if answers.len() == questions.len() => {
                        Ok(format_answers(&questions, &answers))
                    }
                    _ => Err(dismissed()),
                }
            }
            ToolCall::WriteFile { path, content } => {
                // Only the built-in scaffold mode may write project config; config
                // loading rejects project modes with that name.
                let write = if self.mode == SCAFFOLD_MODE {
                    tfs::prepare_scaffold_write(&self.guard, &path, &content)?
                } else {
                    tfs::prepare_write(&self.guard, &path, &content)?
                };
                self.apply_write(write).await
            }
            ToolCall::EditFile {
                path,
                old_string,
                new_string,
                replace_all,
            } => {
                let write = if self.mode == SCAFFOLD_MODE {
                    tfs::prepare_scaffold_edit(
                        &self.guard,
                        &path,
                        &old_string,
                        &new_string,
                        replace_all,
                    )?
                } else {
                    tfs::prepare_edit(&self.guard, &path, &old_string, &new_string, replace_all)?
                };
                self.apply_write(write).await
            }
            ToolCall::RunCommand { name, reason } => self.run_command(&name, &reason).await,
            // The model's own patterns and replacements could add invisible characters, so
            // results are filtered again.
            ToolCall::VarGrep {
                var,
                pattern,
                context,
                invert,
                only_matching,
            } => {
                let re = vars::regex(&pattern)?;
                let text = &self.vars.get(&var)?.text;
                Ok(reveal(vars::grep(
                    text,
                    &re,
                    context,
                    invert,
                    only_matching,
                )))
            }
            ToolCall::VarReplace {
                var,
                pattern,
                replacement,
            } => {
                let re = vars::regex(&pattern)?;
                let text = &self.vars.get(&var)?.text;
                Ok(reveal(vars::replace(text, &re, &replacement)))
            }
            ToolCall::VarLines { var, options } => {
                Ok(vars::lines(&self.vars.get(&var)?.text, &options))
            }
            ToolCall::VarShow { var } => Ok(self.vars.get(&var)?.text.clone()),
            ToolCall::VarList { drop } => {
                let missing = self.vars.drop_vars(&drop);
                let mut out = self.vars.list();
                if !missing.is_empty() {
                    out = format!("No such variable: {}\n{out}", missing.join(", "));
                }
                Ok(out)
            }
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
            WriteClass::AgentsConfig => Some(
                "Project config: it decides which commands and modes the agent gets. Cradle \
                 asks you to trust the change before your next message."
                    .to_owned(),
            ),
        };
        let edits = self
            .modes
            .get(&self.mode)
            .map_or(FileEdits::Ask, |m| m.effective_edits());
        // Protected files and .agents/ always ask; other files as the mode says.
        let ask_always = note.is_none() && edits == FileEdits::Ask && !self.edits_allowed;
        if note.is_some() || ask_always {
            let verb = if write.existed { "Change" } else { "Create" };
            let always = ask_always.then(|| "allow all file edits this session".to_owned());
            match self
                .ask(format!("{verb} {rel}?"), write.diff.clone(), note, always)
                .await
            {
                Approval::Yes => {}
                Approval::Always => self.edits_allowed = true,
                Approval::No => {
                    return Err(ToolError::new(
                        "The user declined this change. Ask them how they want to proceed.",
                    ));
                }
                Approval::Change(feedback) => {
                    return Err(ToolError::new(format!(
                        "The user did not apply this change and asks for something different: \
                         {feedback}"
                    )));
                }
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
        self.edited = true;

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
        self.emit(AgentEvent::FilesChanged(self.file_stats()));
        Ok(summary)
    }

    async fn run_command(&mut self, name: &str, reason: &str) -> Result<String, ToolError> {
        let mode = self.modes.get(&self.mode).expect("current mode exists");
        let mode_line = mode.commands.get(name).cloned();
        let read_only = mode.access == ModeAccess::Read;
        // The mode's own definition wins; read-only modes have nothing else.
        let line = match &mode_line {
            Some(line) => line.clone(),
            None => match self.commands.get(name) {
                Some(line) if !read_only => line.clone(),
                _ => {
                    let mut names: BTreeSet<&str> =
                        mode.commands.keys().map(String::as_str).collect();
                    if !read_only {
                        names.extend(self.commands.keys().map(String::as_str));
                    }
                    let names: Vec<&str> = names.into_iter().collect();
                    return Err(ToolError::new(format!(
                        "command `{name}` is not available in `{}` mode; available: {}",
                        mode.name,
                        if names.is_empty() {
                            "none".into()
                        } else {
                            names.join(", ")
                        }
                    )));
                }
            },
        };
        // A line the mode reviewer did not pass is blocked wherever it is configured.
        if let Some(why) = self.blocked.get(&line) {
            return Err(ToolError::new(format!(
                "`{name}` is blocked for this session: the reviewer did not judge `{line}` \
                 safe ({why}). Do not retry; tell the user."
            )));
        }
        let argv = command::split(&line)?;
        let reason = reveal_invisible(reason).into_owned();

        let reviewed_safe = mode_line.is_some()
            && self.review == ReviewPolicy::Subagent
            && self.mode_safe.contains(&line);
        let pre_approved = (self.always_allowed.contains(&line) || reviewed_safe)
            && self.protected_changed.is_empty();
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
                let always = Some("always this session".to_owned());
                match self.ask(format!("Run `{name}`?"), body, note, always).await {
                    Approval::Yes => {
                        self.protected_changed.clear();
                    }
                    Approval::Always => {
                        self.protected_changed.clear();
                        self.always_allowed.insert(line.clone());
                    }
                    Approval::No => {
                        return Err(ToolError::new(format!(
                            "The user declined to run `{name}`."
                        )));
                    }
                    Approval::Change(feedback) => {
                        return Err(ToolError::new(format!(
                            "The user did not run `{name}` and says: {feedback}"
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

    /// Lines added and removed per changed file, for the sidebar.
    fn file_stats(&self) -> Vec<FileStat> {
        self.changed_files()
            .into_iter()
            .map(|f| {
                let (added, removed) = diff_counts(&f.diff);
                FileStat {
                    path: f.path,
                    added,
                    removed,
                }
            })
            .collect()
    }

    async fn ask(
        &self,
        title: String,
        body: String,
        note: Option<String>,
        always: Option<String>,
    ) -> Approval {
        let (reply, answer) = oneshot::channel();
        self.emit(AgentEvent::Approval(ApprovalRequest {
            title,
            body,
            note,
            always,
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

/// The preferences that apply for `project`, using the user's stored choices, and the
/// project preferences still to ask about.
fn resolve_soul(
    user: &Soul,
    project: Option<&ProjectConfig>,
    root: &Path,
    store: Option<&TrustStore>,
) -> (Soul, Vec<SoulKey>) {
    let empty = SoulPatch::default();
    let patch = project.map_or(&empty, |p| &p.soul);
    prefs::resolve(user, patch, |key, value| {
        store.and_then(|s| s.soul_choice(root, key, value))
    })
}

/// The tool definitions for a project. `run_command` lists `[commands]` and every mode's
/// own commands; which of them may run in the current mode is enforced per call.
fn tool_set(project: Option<&ProjectConfig>) -> (Vec<ToolDefinition>, Vec<ToolSpec>) {
    let mut listed: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    if let Some(project) = project {
        for (name, line) in &project.commands {
            listed.entry(name).or_default().push(line.clone());
        }
        for (mode, commands) in &project.modes.commands {
            for (name, line) in commands {
                listed
                    .entry(name)
                    .or_default()
                    .push(format!("{line} (`{mode}` mode command)"));
            }
        }
    }
    let lines: Vec<(&str, String)> = listed
        .into_iter()
        .map(|(name, lines)| (name, lines.join("; ")))
        .collect();
    let pairs: Vec<(&str, &str)> = lines.iter().map(|(n, l)| (*n, l.as_str())).collect();
    let tools = cradle_tools::definitions(&pairs);
    let specs = tools
        .iter()
        .map(|t| ToolSpec {
            name: t.name.to_owned(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
        })
        .collect();
    (tools, specs)
}

/// Load the project config as it is on disk now. When both forms exist, use the stored
/// choice, else the form in use. `Ok(None)`: there is no project config.
fn load_project(
    root: &Path,
    store: Option<&TrustStore>,
    current: Option<SourceKind>,
) -> Result<Option<ProjectConfig>, String> {
    let source = match cradle_config::detect(root).map_err(|e| e.to_string())? {
        Detection::None => return Ok(None),
        Detection::One(source) => source,
        Detection::Both => match store.and_then(|s| s.source_choice(root)).or(current) {
            Some(source) => source,
            None => {
                return Err(
                    "both .agents/ and agents.toml exist; restart Cradle to choose one".into(),
                );
            }
        },
    };
    cradle_config::load(root, source)
        .map(Some)
        .map_err(|e| e.to_string())
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

/// Added and removed lines in a unified diff, ignoring the file headers.
fn diff_counts(diff: &str) -> (usize, usize) {
    let mut counts = (0, 0);
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            counts.0 += 1;
        } else if line.starts_with('-') {
            counts.1 += 1;
        }
    }
    counts
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
    fn counts_diff_lines() {
        let diff = "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n-old\n+new\n+more\n same\n";
        assert_eq!(diff_counts(diff), (2, 1));
    }

    #[test]
    fn formats_answers() {
        let single = Question::single("Which?", vec!["a".into(), "b".into()]);
        assert_eq!(
            format_answers(std::slice::from_ref(&single), &[Answer::Choice("a".into())]),
            "The user answered: a"
        );
        assert_eq!(
            format_answers(
                std::slice::from_ref(&single),
                &[Answer::Text("neither".into())]
            ),
            "The user answered: (own answer) neither"
        );

        let multi = Question {
            question: "Which ones?".into(),
            kind: QuestionKind::Multi,
            options: vec!["a".into(), "b".into(), "c".into()],
        };
        let text = Question {
            question: "Why?".into(),
            kind: QuestionKind::Text,
            options: Vec::new(),
        };
        let answers = [
            Answer::Choice("b".into()),
            Answer::Choices {
                picked: vec!["a".into(), "c".into()],
                other: Some("d".into()),
            },
            Answer::Text("because".into()),
        ];
        assert_eq!(
            format_answers(&[single, multi, text], &answers),
            "The user answered:\n1. Which?\n   b\n2. Which ones?\n   a, c; other: \"d\"\n\
             3. Why?\n   because"
        );
    }

    #[test]
    fn previews_are_short() {
        assert_eq!(preview("a\nb"), "a\nb");
        assert_eq!(preview("1\n2\n3\n4\n5\n6"), "1\n2\n3\n4\n… 2 more lines");
    }
}

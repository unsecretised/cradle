//! The terminal UI: a transcript, an input box, and modals for approvals and questions.

mod commands;
mod markdown;
mod onboard;
mod picker;
mod questions;
mod text;
mod tools;
mod view;

use std::cell::Cell;
use std::io::{Write, stdout};
use std::path::PathBuf;
use std::time::Duration;

use cradle_backends::{BackendError, Block, Message, Role, Usage};
use cradle_core::session::SessionInfo;
use cradle_core::{
    AgentCommand, AgentEvent, AgentHandle, Approval, FileStat, ReviewPolicy, Verdict,
};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEvent, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use futures_util::StreamExt;
use tokio::sync::oneshot;

use cradle_backends::{Backend, available_models};
use cradle_config::user::UserConfig;
use cradle_provider_spec::{Backend as SpecBackend, ProviderSpec};
use tokio::sync::mpsc;

use crate::Error;
use crate::setup::Session;
use cradle_core::AgentSetup;
use picker::{ModelPicker, PickerAction};
use questions::Questions;
use text::Input;

/// Mouse button and wheel reports (SGR encoding), without motion tracking, which would
/// redraw on every mouse move.
const ENABLE_MOUSE: &[u8] = b"\x1b[?1000h\x1b[?1006h";
const DISABLE_MOUSE: &[u8] = b"\x1b[?1006l\x1b[?1000l";
const SCROLL_LINES: usize = 3;

pub async fn run(session: Session) -> Result<(), Error> {
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste);
    let _ = stdout()
        .write_all(ENABLE_MOUSE)
        .and_then(|()| stdout().flush());
    // Where supported (kitty protocol), Shift+Enter and Cmd+Backspace are reported.
    let enhanced = crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false)
        && execute!(
            stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok();
    let result = start(&mut terminal, session).await;
    if enhanced {
        let _ = execute!(stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = stdout()
        .write_all(DISABLE_MOUSE)
        .and_then(|()| stdout().flush());
    let _ = execute!(stdout(), DisableBracketedPaste);
    ratatui::restore();
    result
}

async fn start(terminal: &mut ratatui::DefaultTerminal, session: Session) -> Result<(), Error> {
    let Session {
        agent,
        root,
        mode_names,
        resumed,
        trusted,
        providers,
        backend,
        preselect,
        user_config,
    } = session;
    let mut keys = EventStream::new();

    let backend = match backend {
        Ok(backend) => backend,
        Err(reason) => {
            let chosen = onboard::run(
                terminal,
                &mut keys,
                &providers,
                reason,
                preselect.as_deref(),
                user_config.as_deref(),
            )
            .await?;
            match chosen {
                Some(backend) => backend,
                None => return Ok(()),
            }
        }
    };

    let setup = agent.with_backend(backend);
    let info = StartInfo {
        root: &root,
        mode_names,
        resumed,
        trusted,
        providers,
        user_config,
    };
    let mut app = App::new(info, &setup);
    app.load_models();
    let mut handle = cradle_core::spawn(setup);
    event_loop(terminal, &mut keys, &mut app, &mut handle).await
}

struct StartInfo<'a> {
    root: &'a std::path::Path,
    mode_names: Vec<String>,
    resumed: usize,
    trusted: bool,
    providers: Vec<ProviderSpec>,
    user_config: Option<std::path::PathBuf>,
}

/// Results of work the UI started in the background.
enum UiEvent {
    /// `provider/model` for every usable provider, and why the current provider's models
    /// could not be listed live.
    Models(Vec<String>, Option<String>),
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    keys: &mut EventStream,
    app: &mut App,
    handle: &mut AgentHandle,
) -> Result<(), Error> {
    let mut tick = tokio::time::interval(Duration::from_millis(120));
    let mut ui_events = app.ui_events.take().expect("UI channel is set up once");
    loop {
        terminal
            .draw(|frame| view::draw(frame, app))
            .map_err(Error::Terminal)?;
        tokio::select! {
            Some(event) = ui_events.recv() => app.on_ui(event),
            event = keys.next() => match event {
                Some(Ok(event)) => app.on_terminal(event, handle),
                Some(Err(e)) => return Err(Error::Terminal(e)),
                None => return Ok(()),
            },
            event = handle.events.recv() => match event {
                Some(event) => {
                    app.on_agent(event);
                    // Apply everything already queued before redrawing.
                    while let Ok(event) = handle.events.try_recv() {
                        app.on_agent(event);
                    }
                }
                None => return Err(Error::Setup("the agent stopped unexpectedly".into())),
            },
            _ = tick.tick() => app.spinner = app.spinner.wrapping_add(1),
        }
        if app.quit {
            return Ok(());
        }
    }
}

pub(crate) enum Item {
    User(String),
    Assistant(String),
    Thinking(String),
    /// One tool call, or a run of grouped ones.
    Tools(tools::ToolGroup),
    Diff {
        title: String,
        diff: String,
    },
    Review {
        command: String,
        verdict: Verdict,
        reason: String,
    },
    Notice(String),
    Error(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolState {
    Running,
    Ok,
    Failed,
}

pub(crate) enum Modal {
    Approval {
        title: String,
        body: String,
        note: Option<String>,
        /// The label for "allow for the rest of the session", if offered.
        always: Option<String>,
        reply: Option<oneshot::Sender<Approval>>,
        scroll: usize,
        /// What to change instead, while the user types it (after `c`).
        feedback: Option<Input>,
    },
    Question(Questions),
    Help,
    Model(ModelPicker),
    Session {
        sessions: Vec<SessionInfo>,
        selected: usize,
    },
}

pub(crate) struct App {
    pub items: Vec<Item>,
    /// Lines scrolled up from the bottom of the transcript.
    pub scroll: usize,
    /// The furthest the transcript can scroll, as of the last draw.
    pub max_scroll: Cell<usize>,
    /// Files changed this session.
    pub files: Vec<FileStat>,
    /// Show the sidebar (when the terminal is wide enough).
    pub sidebar: bool,
    /// Show each file and pattern in grouped tool cards (Ctrl+O).
    pub tools_expanded: bool,
    pub input: Input,
    pub modal: Option<Modal>,
    pub busy: bool,
    pub mode: String,
    pub read_only: bool,
    pub review: ReviewPolicy,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    pub spinner: usize,
    pub mode_names: Vec<String>,
    sent: Vec<String>,
    sent_pos: Option<usize>,
    quit: bool,
    quit_armed: bool,
    providers: Vec<ProviderSpec>,
    user_config: Option<std::path::PathBuf>,
    root: PathBuf,
    data_dir: Option<PathBuf>,
    ui: mpsc::UnboundedSender<UiEvent>,
    ui_events: Option<mpsc::UnboundedReceiver<UiEvent>>,
    /// Mode names and whether each is read-only, for suggestions.
    pub modes: Vec<(String, bool)>,
    /// The project's skills (`.agents/skills/`).
    skills: Vec<cradle_config::Skill>,
    /// `provider/model` offered by `/model` (configured, then any listed live).
    known_models: Vec<String>,
    /// Whether `known_models` has been listed live yet.
    models_loaded: bool,
    models_error: Option<String>,
    pub suggest_index: usize,
    /// Esc hides suggestions until the input changes.
    suggest_hidden: bool,
}

impl App {
    fn new(info: StartInfo<'_>, setup: &AgentSetup) -> Self {
        let (ui, ui_events) = mpsc::unbounded_channel();
        let modes = setup
            .modes
            .names()
            .map(|name| {
                let read_only = setup
                    .modes
                    .get(name)
                    .is_some_and(|m| m.access == cradle_config::ModeAccess::Read);
                (name.to_owned(), read_only)
            })
            .collect();
        let known_models = info
            .providers
            .iter()
            .find(|p| p.id == setup.backend.provider())
            .map(|p| configured_models(p).collect())
            .unwrap_or_default();
        let mut items = vec![Item::Notice(format!(
            "cradle in {} · /help for commands",
            info.root.display()
        ))];
        if !info.trusted {
            items.push(Item::Notice(
                "Project config is not trusted (or absent): no project commands or modes.".into(),
            ));
        }
        if info.resumed > 0 {
            items.extend(history_items(&setup.history));
            items.push(Item::Notice(format!(
                "Resumed a conversation with {} messages.",
                info.resumed
            )));
        }
        App {
            items,
            scroll: 0,
            max_scroll: Cell::new(0),
            files: Vec::new(),
            sidebar: true,
            tools_expanded: false,
            input: Input::default(),
            modal: None,
            busy: false,
            mode: setup.mode.clone(),
            read_only: false,
            review: setup.review,
            provider: setup.backend.provider().to_owned(),
            model: setup.backend.model().to_owned(),
            usage: Usage::default(),
            spinner: 0,
            mode_names: info.mode_names,
            sent: Vec::new(),
            sent_pos: None,
            quit: false,
            quit_armed: false,
            providers: info.providers,
            user_config: info.user_config,
            root: info.root.to_path_buf(),
            data_dir: setup.data_dir.clone(),
            ui,
            ui_events: Some(ui_events),
            modes,
            skills: setup
                .project
                .as_ref()
                .map(|p| p.skills.clone())
                .unwrap_or_default(),
            known_models,
            models_loaded: false,
            models_error: None,
            suggest_index: 0,
            suggest_hidden: false,
        }
    }

    fn on_agent(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::Text(t) => match self.items.last_mut() {
                Some(Item::Assistant(text)) => text.push_str(&t),
                _ => self.items.push(Item::Assistant(t)),
            },
            AgentEvent::Thinking(t) => match self.items.last_mut() {
                Some(Item::Thinking(text)) => text.push_str(&t),
                _ => self.items.push(Item::Thinking(t)),
            },
            AgentEvent::ToolStarted {
                id,
                summary,
                activity,
            } => tools::push(
                &mut self.items,
                tools::ToolEntry::new(id, summary, activity),
            ),
            AgentEvent::ToolFinished {
                id,
                ok,
                preview,
                failed,
            } => tools::finish(&mut self.items, &id, ok, preview, failed),
            AgentEvent::Diff { title, diff } => self.items.push(Item::Diff { title, diff }),
            AgentEvent::ReviewResult {
                command,
                verdict,
                reason,
            } => self.items.push(Item::Review {
                command,
                verdict,
                reason,
            }),
            AgentEvent::Approval(req) => {
                self.modal = Some(Modal::Approval {
                    title: req.title,
                    body: req.body,
                    note: req.note,
                    always: req.always,
                    reply: Some(req.reply),
                    scroll: 0,
                    feedback: None,
                })
            }
            AgentEvent::Question(req) => {
                self.modal = Some(Modal::Question(Questions::new(req.questions, req.reply)))
            }
            AgentEvent::ModeChanged { name, read_only } => {
                if name != self.mode {
                    self.items.push(Item::Notice(format!(
                        "Mode: {name}{}",
                        if read_only { " (read-only)" } else { "" }
                    )));
                }
                self.mode = name;
                self.read_only = read_only;
            }
            AgentEvent::ModesChanged(modes) => {
                self.mode_names = modes.iter().map(|(name, _)| name.clone()).collect();
                self.modes = modes;
            }
            AgentEvent::ReviewChanged(policy) => self.review = policy,
            AgentEvent::ModelChanged { provider, model } => {
                self.items
                    .push(Item::Notice(format!("Model: {provider}/{model}")));
                self.provider = provider;
                self.model = model;
            }
            AgentEvent::Usage(usage) => self.usage = usage,
            AgentEvent::FilesChanged(files) => self.files = files,
            AgentEvent::SessionLoaded(history) => {
                self.items = history_items(&history);
                self.items.push(Item::Notice(format!(
                    "Resumed a conversation with {} messages.",
                    history.len()
                )));
                self.scroll = 0;
            }
            AgentEvent::Notice(n) => self.items.push(Item::Notice(n)),
            AgentEvent::Error(e) => self.items.push(Item::Error(e)),
            AgentEvent::TurnFinished => {
                self.busy = false;
                // A cancelled turn may leave a modal whose request is gone.
                if !matches!(self.modal, Some(Modal::Help)) {
                    self.modal = None;
                }
            }
        }
    }

    fn on_terminal(&mut self, event: Event, handle: &AgentHandle) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key, handle),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            Event::Paste(text) => match &mut self.modal {
                Some(Modal::Question(Questions { input, .. }))
                | Some(Modal::Approval {
                    feedback: Some(input),
                    ..
                }) => input.insert_str(&text.replace('\r', "")),
                Some(Modal::Model(picker)) => picker.insert_str(&text),
                Some(_) => {}
                None => self.input.insert_str(&text.replace('\r', "")),
            },
            _ => {}
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let up = match mouse.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return,
        };
        match &mut self.modal {
            Some(Modal::Approval { scroll, .. }) if up => *scroll = scroll.saturating_sub(1),
            Some(Modal::Approval { scroll, .. }) => *scroll += 1,
            Some(_) => {}
            None if up => self.scroll_by(SCROLL_LINES as isize),
            None => self.scroll_by(-(SCROLL_LINES as isize)),
        }
    }

    /// Scroll the transcript; positive is up (older).
    fn scroll_by(&mut self, lines: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(lines)
            .min(self.max_scroll.get());
    }

    fn cycle_mode(&mut self, back: bool, handle: &AgentHandle) {
        if self.busy {
            self.note("Wait for the turn to finish before switching modes.");
        } else if back {
            send(handle, AgentCommand::CycleModeBack);
        } else {
            send(handle, AgentCommand::CycleMode);
        }
    }

    fn on_key(&mut self, key: KeyEvent, handle: &AgentHandle) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.on_ctrl_c(handle);
            return;
        }
        self.quit_armed = false;
        if ctrl && key.code == KeyCode::Char('d') && self.input.is_empty() {
            self.quit = true;
            return;
        }

        if self.modal.is_some() {
            self.on_modal_key(key, handle);
            return;
        }
        if self.on_suggestion_key(key.code, handle) {
            return;
        }

        let before = self.input.text().to_owned();
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let cmd = key.modifiers.contains(KeyModifiers::SUPER);
        match key.code {
            // Terminals without the kitty protocol send Shift+Enter as plain Enter; some
            // are set up to send Alt+Enter or Ctrl+J instead.
            KeyCode::Enter if shift || alt => self.input.insert('\n'),
            KeyCode::Char('j') if ctrl => self.input.insert('\n'),
            KeyCode::Enter => self.submit(handle),
            KeyCode::Esc if self.busy => handle.cancel(),
            KeyCode::Tab if shift => self.cycle_mode(true, handle),
            KeyCode::Tab => self.cycle_mode(false, handle),
            KeyCode::BackTab => self.cycle_mode(true, handle),
            // Cmd+Backspace arrives as Ctrl+U in most macOS terminals.
            KeyCode::Char('u') if ctrl => self.input.delete_to_line_start(),
            KeyCode::Backspace if cmd => self.input.delete_to_line_start(),
            KeyCode::Char('w') if ctrl => self.input.delete_word(),
            KeyCode::Char('o') if ctrl => self.tools_expanded = !self.tools_expanded,
            KeyCode::Backspace if alt || ctrl => self.input.delete_word(),
            KeyCode::Char(_) if ctrl || cmd => {}
            KeyCode::Char(c) => self.input.insert(c),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Up if !self.input.text().contains('\n') => self.recall(-1),
            KeyCode::Down if !self.input.text().contains('\n') => self.recall(1),
            KeyCode::PageUp => self.scroll_by(10),
            KeyCode::PageDown => self.scroll_by(-10),
            _ => {}
        }
        if self.input.text() != before {
            self.suggest_index = 0;
            self.suggest_hidden = false;
            // Editing a recalled message makes it a new one.
            if !matches!(key.code, KeyCode::Up | KeyCode::Down) {
                self.sent_pos = None;
            }
        }
    }

    /// Suggestions for the current input (empty when hidden or not a slash command).
    pub fn suggestions(&self) -> Vec<commands::Suggestion> {
        // While ↑↓ walk through earlier messages, a recalled command shows no suggestions,
        // so ↑↓ keep walking.
        if self.suggest_hidden || self.modal.is_some() || self.sent_pos.is_some() {
            return Vec::new();
        }
        let current = format!("{}/{}", self.provider, self.model);
        let skills: Vec<(String, String)> = self
            .skills
            .iter()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect();
        let ctx = commands::Context {
            skills: &skills,
            modes: &self.modes,
            current_mode: &self.mode,
            models: &self.known_models,
            current_model: &current,
        };
        commands::suggest(self.input.text(), &ctx)
    }

    /// Keys that act on the suggestion list. Returns whether the key was used.
    fn on_suggestion_key(&mut self, code: KeyCode, handle: &AgentHandle) -> bool {
        let suggestions = self.suggestions();
        if suggestions.is_empty() {
            return false;
        }
        let last = suggestions.len() - 1;
        let selected = &suggestions[self.suggest_index.min(last)];
        match code {
            KeyCode::Up => self.suggest_index = self.suggest_index.min(last).saturating_sub(1),
            KeyCode::Down => self.suggest_index = (self.suggest_index + 1).min(last),
            KeyCode::Tab => {
                self.input.set(selected.complete.clone());
                self.suggest_index = 0;
            }
            KeyCode::Enter => {
                match &selected.run {
                    Some(line) => {
                        self.input.set(line.clone());
                        self.submit(handle);
                    }
                    None => self.input.set(selected.complete.clone()),
                }
                self.suggest_index = 0;
            }
            KeyCode::Esc => self.suggest_hidden = true,
            _ => return false,
        }
        true
    }

    fn on_ctrl_c(&mut self, handle: &AgentHandle) {
        if !self.input.is_empty() && self.modal.is_none() {
            self.input.take();
        } else if self.busy {
            handle.cancel();
        } else if self.quit_armed {
            self.quit = true;
        } else {
            self.quit_armed = true;
            self.note("Press Ctrl+C again to quit.");
        }
    }

    fn on_modal_key(&mut self, key: KeyEvent, handle: &AgentHandle) {
        let Some(modal) = &mut self.modal else { return };
        match modal {
            Modal::Session { sessions, selected } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => self.modal = None,
                KeyCode::Up | KeyCode::Char('k') => *selected = selected.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => {
                    *selected = (*selected + 1).min(sessions.len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if let Some(session) = sessions.get(*selected) {
                        send(handle, AgentCommand::LoadSession(session.path.clone()));
                    }
                    self.modal = None;
                }
                _ => {}
            },
            Modal::Model(picker) => match picker.on_key(key.code) {
                PickerAction::Cancel => self.modal = None,
                PickerAction::Chosen(model) => {
                    self.modal = None;
                    // `None` (the CLI's default) is offered as `provider/default` here.
                    if let Some(model) = model {
                        self.switch_model(&model, handle);
                    }
                }
                PickerAction::None => {}
            },
            Modal::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.modal = None;
                }
            }
            Modal::Approval {
                reply,
                feedback: Some(input),
                ..
            } => {
                let decision = match key.code {
                    KeyCode::Esc => {
                        if let Some(Modal::Approval { feedback, .. }) = &mut self.modal {
                            *feedback = None;
                        }
                        return;
                    }
                    KeyCode::Enter
                        if key
                            .modifiers
                            .intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
                    {
                        input.insert('\n');
                        None
                    }
                    KeyCode::Enter if !input.text().trim().is_empty() => {
                        Some(Approval::Change(input.take().trim().to_owned()))
                    }
                    KeyCode::Char(c) => {
                        input.insert(c);
                        None
                    }
                    KeyCode::Backspace => {
                        input.backspace();
                        None
                    }
                    KeyCode::Left => {
                        input.left();
                        None
                    }
                    KeyCode::Right => {
                        input.right();
                        None
                    }
                    _ => None,
                };
                if let Some(decision) = decision {
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(decision);
                    }
                    self.modal = None;
                }
            }
            Modal::Approval {
                always,
                reply,
                scroll,
                feedback,
                ..
            } => {
                let decision = match key.code {
                    KeyCode::Char('y') => Some(Approval::Yes),
                    KeyCode::Char('a') if always.is_some() => Some(Approval::Always),
                    KeyCode::Char('c') => {
                        *feedback = Some(Input::default());
                        None
                    }
                    KeyCode::Char('n') | KeyCode::Esc => Some(Approval::No),
                    KeyCode::Down | KeyCode::Char('j') => {
                        *scroll += 1;
                        None
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        *scroll = scroll.saturating_sub(1);
                        None
                    }
                    KeyCode::PageDown => {
                        *scroll += 15;
                        None
                    }
                    KeyCode::PageUp => {
                        *scroll = scroll.saturating_sub(15);
                        None
                    }
                    _ => None,
                };
                if let Some(decision) = decision {
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(decision);
                    }
                    self.modal = None;
                }
            }
            Modal::Question(questions) => {
                // Closing without sending drops the sender, which tells the agent the
                // questions were dismissed.
                if questions.on_key(key) {
                    self.modal = None;
                }
            }
        }
    }

    fn submit(&mut self, handle: &AgentHandle) {
        let text = self.input.text().trim().to_owned();
        if text.is_empty() {
            return;
        }
        if let Some(command) = text.strip_prefix('/') {
            self.input.take();
            self.remember(&text);
            self.slash(command, handle);
            return;
        }
        if self.busy {
            self.note("Still working: Esc cancels the current turn.");
            return;
        }
        self.input.take();
        self.remember(&text);
        self.items.push(Item::User(text.clone()));
        self.scroll = 0;
        self.busy = true;
        send(handle, AgentCommand::Send(text));
    }

    fn slash(&mut self, command: &str, handle: &AgentHandle) {
        let mut words = command.split_whitespace();
        let name = commands::canonical(words.next().unwrap_or(""));
        let arg = words.next();
        // Everything after the command name, for commands that take free text.
        let rest = command
            .trim_start()
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest.trim());
        let idle_only = matches!(
            name,
            "mode" | "plan" | "build" | "clear" | "diff" | "model" | "session" | "skill"
        );
        if idle_only && self.busy {
            self.note("Wait for the turn to finish (or Esc to cancel) first.");
            return;
        }
        match (name, arg) {
            ("help", _) => self.modal = Some(Modal::Help),
            ("quit", _) => self.quit = true,
            ("mode", None) => {
                let names = self.mode_names.join(", ");
                self.note(&format!("Current mode: {}. Available: {names}.", self.mode));
            }
            ("mode", Some(m)) => send(handle, AgentCommand::SetMode(m.to_owned())),
            ("plan" | "build", None) => send(handle, AgentCommand::SetMode(name.to_owned())),
            ("review", Some("user")) => send(handle, AgentCommand::SetReview(ReviewPolicy::User)),
            ("review", Some("subagent")) => {
                send(handle, AgentCommand::SetReview(ReviewPolicy::Subagent))
            }
            ("review", _) => self.note("Usage: /review user | /review subagent"),
            ("diff", _) => send(handle, AgentCommand::ShowDiff),
            ("model", None) => self.open_model_picker(),
            ("model", Some(model)) => self.switch_model(model, handle),
            ("session", _) => self.open_sessions(),
            ("skill", None) => {
                let names: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
                if names.is_empty() {
                    self.note("No skills: add them to .agents/skills/<name>/SKILL.md.");
                } else {
                    self.note(&format!(
                        "Skills: {}. Use /skill <name> [request].",
                        names.join(", ")
                    ));
                }
            }
            ("skill", Some(skill)) => {
                let request = rest.strip_prefix(skill).unwrap_or("").trim().to_owned();
                self.use_skill(skill, &request, handle);
            }
            ("sidebar", _) => self.sidebar = !self.sidebar,
            ("clear", _) => {
                self.items.clear();
                self.usage = Usage::default();
                send(handle, AgentCommand::Clear);
            }
            _ => self.note(&format!("Unknown command /{name}. Try /help.")),
        }
    }

    fn on_ui(&mut self, event: UiEvent) {
        match event {
            UiEvent::Models(models, error) => {
                self.known_models = models.clone();
                self.models_loaded = true;
                self.models_error = error.clone();
                if let Some(Modal::Model(picker)) = &mut self.modal {
                    picker.set_models(models, error);
                }
            }
        }
    }

    /// List every usable provider's models in the background. The current provider's
    /// come first.
    fn load_models(&self) {
        let mut specs = self.providers.clone();
        specs.sort_by_key(|p| p.id != self.provider);
        let current = self.provider.clone();
        let ui = self.ui.clone();
        tokio::spawn(async move {
            let lists = futures_util::future::join_all(specs.iter().map(provider_models)).await;
            let mut models = Vec::new();
            let mut error = None;
            for (spec, (list, e)) in specs.iter().zip(lists) {
                if spec.id == current {
                    error = e.map(|e| e.to_string());
                }
                models.extend(list.into_iter().map(|m| format!("{}/{m}", spec.id)));
            }
            let _ = ui.send(UiEvent::Models(models, error));
        });
    }

    /// Open the picker over every usable provider's models.
    fn open_model_picker(&mut self) {
        let current = format!("{}/{}", self.provider, self.model);
        let mut picker = ModelPicker::new(false, Some(&current));
        if self.models_loaded {
            picker.set_models(self.known_models.clone(), self.models_error.clone());
        } else {
            self.load_models();
        }
        self.modal = Some(Modal::Model(picker));
    }

    /// Send the skill's instructions as a user message, with the user's request.
    fn use_skill(&mut self, name: &str, request: &str, handle: &AgentHandle) {
        let Some(skill) = self.skills.iter().find(|s| s.name == name) else {
            self.note(&format!("No skill named `{name}`. /skill lists them."));
            return;
        };
        let body = match skill.body() {
            Ok(body) => body,
            Err(e) => {
                self.items.push(Item::Error(format!(
                    "cannot read {}: {e}",
                    skill.path.display()
                )));
                return;
            }
        };
        let mut message = format!("Use the `{name}` skill:\n\n<skill>\n{body}\n</skill>");
        if !request.is_empty() {
            message.push_str(&format!("\n\n{request}"));
        }
        let shown = if request.is_empty() {
            format!("/skill {name}")
        } else {
            format!("/skill {name} {request}")
        };
        self.items.push(Item::User(shown));
        self.scroll = 0;
        self.busy = true;
        send(handle, AgentCommand::Send(message));
    }

    fn open_sessions(&mut self) {
        let Some(dir) = &self.data_dir else {
            self.note("Conversations are not saved: no data directory.");
            return;
        };
        match cradle_core::session::list(dir, &self.root) {
            Ok(sessions) if sessions.is_empty() => {
                self.note("No saved conversations in this project yet.")
            }
            Ok(sessions) => {
                self.modal = Some(Modal::Session {
                    sessions,
                    selected: 0,
                })
            }
            Err(e) => self
                .items
                .push(Item::Error(format!("cannot list conversations: {e}"))),
        }
    }

    /// Switch to `provider/model` (or a model on the current provider) and save it as the
    /// default. `default` on a CLI provider means the CLI's own default.
    fn switch_model(&mut self, choice: &str, handle: &AgentHandle) {
        let (id, model) = match choice.split_once('/') {
            Some((id, model)) if self.providers.iter().any(|p| p.id == id) => (id, model),
            _ => (self.provider.as_str(), choice),
        };
        let Some(spec) = self.providers.iter().find(|p| p.id == id).cloned() else {
            self.note("The current provider is not in the provider list.");
            return;
        };
        let is_cli = matches!(spec.backend, SpecBackend::Cli(_));
        let model = (!(is_cli && model == "default")).then(|| model.to_owned());
        match Backend::new(&spec, model.as_deref()) {
            Ok(backend) => {
                send(handle, AgentCommand::SetBackend(Box::new(backend)));
                if let Some(path) = &self.user_config {
                    let mut config = UserConfig::load(path).unwrap_or_default();
                    config.default.provider = Some(spec.id.clone());
                    config.default.model = model;
                    // Not fatal: the switch still applies to this session.
                    let _ = config.save(path);
                }
            }
            Err(e) => self.items.push(Item::Error(e.to_string())),
        }
    }

    fn remember(&mut self, text: &str) {
        if self.sent.last().map(String::as_str) != Some(text) {
            self.sent.push(text.to_owned());
        }
        self.sent_pos = None;
    }

    /// Move through previously sent inputs. `step` is -1 (older) or 1 (newer).
    fn recall(&mut self, step: isize) {
        if self.sent.is_empty() {
            return;
        }
        let last = self.sent.len() - 1;
        let next = match (self.sent_pos, step) {
            (None, -1) => Some(last),
            (None, _) => None,
            (Some(0), -1) => Some(0),
            (Some(i), -1) => Some(i - 1),
            (Some(i), _) if i < last => Some(i + 1),
            (Some(_), _) => None,
        };
        self.sent_pos = next;
        self.input
            .set(next.map(|i| self.sent[i].clone()).unwrap_or_default());
    }

    fn note(&mut self, text: &str) {
        self.items.push(Item::Notice(text.to_owned()));
    }
}

fn send(handle: &AgentHandle, command: AgentCommand) {
    let _ = handle.commands.send(command);
}

/// A provider's configured models; CLI providers also offer their own `default`.
fn configured_models(spec: &ProviderSpec) -> impl Iterator<Item = String> + '_ {
    let (default, models) = match &spec.backend {
        SpecBackend::Http(h) => (None, &h.models),
        SpecBackend::Cli(c) => (Some("default".to_owned()), &c.models),
    };
    default
        .into_iter()
        .chain(models.iter().cloned())
        .map(move |m| format!("{}/{m}", spec.id))
}

/// The models `spec` offers, or none if it can't be used (no key, CLI not installed).
async fn provider_models(spec: &ProviderSpec) -> (Vec<String>, Option<BackendError>) {
    match &spec.backend {
        SpecBackend::Cli(cli) => {
            let binary = cli.binary.clone();
            let installed = tokio::task::spawn_blocking(move || {
                cradle_backends::cli::installed_version(&binary)
            })
            .await
            .ok()
            .flatten()
            .is_some();
            if !installed {
                return (Vec::new(), None);
            }
            let mut models = vec!["default".to_owned()];
            models.extend(cli.models.iter().cloned());
            (models, None)
        }
        SpecBackend::Http(_) => match available_models(spec).await {
            (_, Some(e @ BackendError::MissingKey(_))) => (Vec::new(), Some(e)),
            result => result,
        },
    }
}

/// Transcript items for a saved conversation: what was said, and which tools ran.
fn history_items(history: &[Message]) -> Vec<Item> {
    let mut items = Vec::new();
    for message in history {
        for block in &message.content {
            match (message.role, block) {
                (Role::User, Block::Text { text }) if !text.trim().is_empty() => {
                    items.push(Item::User(text.clone()))
                }
                (Role::Assistant, Block::Text { text }) if !text.trim().is_empty() => {
                    items.push(Item::Assistant(text.clone()))
                }
                (
                    Role::Assistant,
                    Block::ToolUse {
                        id, name, input, ..
                    },
                ) => {
                    let (summary, activity) = match cradle_tools::parse_call(name, input) {
                        Ok(call) => (call.summary(), Some(call.activity())),
                        Err(_) => (name.clone(), None),
                    };
                    let mut entry = tools::ToolEntry::new(id.clone(), summary, activity);
                    entry.state = ToolState::Ok;
                    tools::push(&mut items, entry);
                }
                _ => {}
            }
        }
    }
    items
}

#[cfg(test)]
mod tests {
    use cradle_core::ModeSet;
    use cradle_provider_spec::{Auth, Capabilities, HttpApi, HttpProvider};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn spec() -> ProviderSpec {
        ProviderSpec {
            id: "local".into(),
            backend: SpecBackend::Http(HttpProvider {
                api: HttpApi::OpenaiCompatible,
                base_url: "http://127.0.0.1:9".into(),
                auth: Auth::None,
                models: vec!["small".into(), "large".into()],
                default_model: None,
                headers: vec![],
                capabilities: Capabilities {
                    tools: true,
                    streaming: false,
                    prompt_caching: false,
                    reasoning: false,
                },
            }),
        }
    }

    /// An app wired to a real agent (the model is never called in these tests).
    fn start(dir: &std::path::Path) -> (App, AgentHandle) {
        let setup = AgentSetup {
            backend: Backend::new(&spec(), Some("small")).unwrap(),
            guard: cradle_tools::PathGuard::new(dir).unwrap(),
            project: None,
            modes: ModeSet::resolve(None, None, cradle_core::ClashChoice::Override).unwrap(),
            mode: "plan".into(),
            review: ReviewPolicy::User,
            history: vec![],
            session: None,
            data_dir: None,
            trust_store: None,
            global_modes: None,
        };
        let info = StartInfo {
            root: dir,
            mode_names: vec!["plan".into(), "build".into()],
            resumed: 0,
            trusted: false,
            providers: vec![spec()],
            user_config: None,
        };
        let app = App::new(info, &setup);
        (app, cradle_core::spawn(setup))
    }

    fn screen(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| view::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(buffer.area.width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn press(app: &mut App, handle: &AgentHandle, code: KeyCode) {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE), handle);
    }

    fn type_text(app: &mut App, handle: &AgentHandle, text: &str) {
        for c in text.chars() {
            press(app, handle, KeyCode::Char(c));
        }
    }

    /// Apply agent events until `done` says the expected one arrived.
    async fn until(app: &mut App, handle: &mut AgentHandle, done: impl Fn(&App) -> bool) {
        while !done(app) {
            let event = tokio::time::timeout(Duration::from_secs(5), handle.events.recv())
                .await
                .expect("agent answered")
                .expect("agent running");
            app.on_agent(event);
        }
    }

    #[tokio::test]
    async fn slash_suggestions_complete_and_run() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, mut handle) = start(dir.path());

        type_text(&mut app, &handle, "/");
        let s = screen(&app);
        assert!(s.contains("/help") && s.contains("/model [name]"), "{s}");
        assert!(s.contains("/review user|subagent"));

        type_text(&mut app, &handle, "mo");
        let s = screen(&app);
        assert!(
            s.contains("/mode [name]") && s.contains("/model [name]"),
            "{s}"
        );
        assert!(!s.contains("/clear"));

        press(&mut app, &handle, KeyCode::Tab);
        assert_eq!(app.input.text(), "/mode ");
        let s = screen(&app);
        assert!(
            s.contains("read-only · current") && s.contains("full access"),
            "{s}"
        );

        press(&mut app, &handle, KeyCode::Down);
        press(&mut app, &handle, KeyCode::Enter);
        assert!(app.input.is_empty());
        until(&mut app, &mut handle, |a| a.mode == "build").await;

        type_text(&mut app, &handle, "/rev");
        press(&mut app, &handle, KeyCode::Enter);
        assert_eq!(
            app.input.text(),
            "/review ",
            "a required argument is completed, not run"
        );
        type_text(&mut app, &handle, "s");
        press(&mut app, &handle, KeyCode::Enter);
        until(&mut app, &mut handle, |a| {
            a.review == ReviewPolicy::Subagent
        })
        .await;
        assert!(screen(&app).contains("commands: reviewer, then you"));

        type_text(&mut app, &handle, "/model ");
        let s = screen(&app);
        assert!(
            s.contains("local/small") && s.contains("local/large"),
            "{s}"
        );
        press(&mut app, &handle, KeyCode::Esc);
        assert!(app.suggestions().is_empty(), "Esc hides the popup");
        assert!(!screen(&app).contains("↑↓ select"));
    }

    #[tokio::test]
    async fn history_walks_past_commands() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, handle) = start(dir.path());
        app.remember("first message");
        app.remember("/help");
        app.remember("last message");
        press(&mut app, &handle, KeyCode::Up);
        press(&mut app, &handle, KeyCode::Up);
        assert_eq!(app.input.text(), "/help");
        assert!(app.suggestions().is_empty());
        press(&mut app, &handle, KeyCode::Up);
        assert_eq!(app.input.text(), "first message");
        press(&mut app, &handle, KeyCode::Down);
        press(&mut app, &handle, KeyCode::Down);
        assert_eq!(app.input.text(), "last message");
    }

    #[tokio::test]
    async fn tab_and_shift_tab_cycle_modes() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, mut handle) = start(dir.path());
        until(&mut app, &mut handle, |a| a.mode == "plan").await;
        press(&mut app, &handle, KeyCode::Tab);
        until(&mut app, &mut handle, |a| a.mode == "build").await;
        press(&mut app, &handle, KeyCode::BackTab);
        until(&mut app, &mut handle, |a| a.mode == "plan").await;
    }

    #[tokio::test]
    async fn sidebar_shows_changed_files() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, _handle) = start(dir.path());
        app.on_agent(AgentEvent::FilesChanged(vec![FileStat {
            path: "src/main.rs".into(),
            added: 12,
            removed: 3,
        }]));
        let s = screen(&app);
        assert!(
            s.contains("Changed files (1)") && s.contains("+12 -3"),
            "{s}"
        );
        app.sidebar = false;
        assert!(!screen(&app).contains("Changed files"));
    }

    #[tokio::test]
    async fn approval_can_ask_for_changes() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, handle) = start(dir.path());
        let (reply, answer) = oneshot::channel();
        app.on_agent(AgentEvent::Approval(cradle_core::ApprovalRequest {
            title: "Create a.txt?".into(),
            body: "+a".into(),
            note: None,
            always: Some("allow all file edits this session".into()),
            reply,
        }));
        let s = screen(&app);
        assert!(
            s.contains("[a] allow all file edits this session") && s.contains("[c] change"),
            "{s}"
        );
        press(&mut app, &handle, KeyCode::Char('c'));
        type_text(&mut app, &handle, "name it b.txt");
        assert!(screen(&app).contains("› name it b.txt"));
        press(&mut app, &handle, KeyCode::Enter);
        assert!(app.modal.is_none());
        assert_eq!(
            answer.await.unwrap(),
            Approval::Change("name it b.txt".into())
        );
    }

    #[tokio::test]
    async fn grouped_tool_calls_collapse_and_expand() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, handle) = start(dir.path());
        let calls = [
            (
                "t1",
                cradle_tools::ToolCall::ReadFile {
                    path: "src/alpha.rs".into(),
                    offset: None,
                    limit: None,
                },
            ),
            (
                "t2",
                cradle_tools::ToolCall::ReadFiles {
                    files: ["src/beta.rs", "src/gamma.rs"]
                        .map(|p| cradle_tools::FileRange {
                            path: p.into(),
                            offset: None,
                            limit: None,
                        })
                        .to_vec(),
                },
            ),
            (
                "t3",
                cradle_tools::ToolCall::Search {
                    pattern: "needle".into(),
                    path: None,
                    glob: None,
                },
            ),
        ];
        for (id, call) in calls {
            app.on_agent(AgentEvent::ToolStarted {
                id: id.into(),
                summary: call.summary(),
                activity: Some(call.activity()),
            });
        }
        let failed = vec!["src/gamma.rs: does not exist".to_owned()];
        for (id, failed) in [("t1", vec![]), ("t2", failed), ("t3", vec![])] {
            app.on_agent(AgentEvent::ToolFinished {
                id: id.into(),
                ok: true,
                preview: "     1\tsecret contents".into(),
                failed,
            });
        }

        let s = screen(&app);
        assert!(
            s.contains("Read 3 files, searched 1 pattern (1 failed)"),
            "{s}"
        );
        assert!(
            !s.contains("src/beta.rs") && !s.contains("secret contents"),
            "{s}"
        );

        app.on_key(
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            &handle,
        );
        let s = screen(&app);
        assert!(
            s.contains("│ src/alpha.rs") && s.contains("│ src/beta.rs"),
            "{s}"
        );
        assert!(s.contains("✗ src/gamma.rs: does not exist"), "{s}");
        assert!(s.contains("│ /needle/ in ."), "{s}");
        assert!(!s.contains("secret contents"), "{s}");

        app.on_key(
            KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL),
            &handle,
        );
        assert!(!screen(&app).contains("src/alpha.rs"));
    }

    fn ask(
        app: &mut App,
        questions: Vec<cradle_tools::Question>,
    ) -> oneshot::Receiver<Vec<cradle_core::Answer>> {
        let (reply, answers) = oneshot::channel();
        app.on_agent(AgentEvent::Question(cradle_core::QuestionRequest {
            questions,
            reply,
        }));
        answers
    }

    fn question(
        text: &str,
        kind: cradle_tools::QuestionKind,
        options: &[&str],
    ) -> cradle_tools::Question {
        cradle_tools::Question {
            question: text.into(),
            kind,
            options: options.iter().map(|o| o.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn questions_step_through_each_kind() {
        use cradle_core::Answer;
        use cradle_tools::QuestionKind as K;
        let dir = tempfile::tempdir().unwrap();
        let (mut app, handle) = start(dir.path());
        let ten: Vec<String> = (1..=10).map(|i| format!("opt{i}")).collect();
        let ten: Vec<&str> = ten.iter().map(String::as_str).collect();
        let answers = ask(
            &mut app,
            vec![
                question("Pick a backend?", K::Single, &["http", "cli"]),
                question("Which crates?", K::Multi, &["core", "tools", "tui"]),
                question("Why?", K::Text, &[]),
                question("Own answer?", K::Single, &["a", "b"]),
                question("Tenth?", K::Single, &ten),
            ],
        );
        let s = screen(&app);
        assert!(
            s.contains("The agent asks (1 of 5)") && s.contains("1. http"),
            "{s}"
        );
        assert!(!s.contains("⇧Tab back"), "{s}");

        // Single: a digit picks and moves on.
        press(&mut app, &handle, KeyCode::Char('2'));
        let s = screen(&app);
        assert!(
            s.contains("(2 of 5)") && s.contains("✓ Pick a backend?: cli"),
            "{s}"
        );
        assert!(s.contains("1. [ ] core") && s.contains("⇧Tab back"), "{s}");

        // Multi: Enter with nothing ticked does nothing; Space and digits toggle.
        press(&mut app, &handle, KeyCode::Enter);
        assert!(screen(&app).contains("(2 of 5)"));
        press(&mut app, &handle, KeyCode::Char(' '));
        press(&mut app, &handle, KeyCode::Char('3'));
        assert!(screen(&app).contains("3. [x] tui"));
        // Once typing, Space and digits are text.
        type_text(&mut app, &handle, "cli 2");
        press(&mut app, &handle, KeyCode::Enter);

        // Text: empty Enter does nothing.
        assert!(screen(&app).contains("(3 of 5)"));
        press(&mut app, &handle, KeyCode::Enter);
        assert!(screen(&app).contains("(3 of 5)"));
        type_text(&mut app, &handle, "speed");

        // Back restores the multi answer, ticks and own text included.
        press(&mut app, &handle, KeyCode::BackTab);
        let s = screen(&app);
        assert!(s.contains("(2 of 5)") && s.contains("1. [x] core"), "{s}");
        assert!(
            s.contains("2. [ ] tools") && s.contains("Or your own: cli 2"),
            "{s}"
        );
        press(&mut app, &handle, KeyCode::Enter);
        assert!(
            screen(&app).contains("Answer: speed"),
            "kept on the way back"
        );
        press(&mut app, &handle, KeyCode::Enter);

        // Single with an own answer.
        type_text(&mut app, &handle, "neither");
        press(&mut app, &handle, KeyCode::Enter);

        // `0` picks the tenth option and sends everything.
        assert!(screen(&app).contains("1-9,0 pick"));
        press(&mut app, &handle, KeyCode::Char('0'));
        assert!(app.modal.is_none());
        assert_eq!(
            answers.await.unwrap(),
            [
                Answer::Choice("cli".into()),
                Answer::Choices {
                    picked: vec!["core".into(), "tui".into()],
                    other: Some("cli 2".into())
                },
                Answer::Text("speed".into()),
                Answer::Text("neither".into()),
                Answer::Choice("opt10".into()),
            ]
        );
    }

    #[tokio::test]
    async fn dismissing_questions_asks_first_once_started() {
        use cradle_tools::QuestionKind as K;
        let dir = tempfile::tempdir().unwrap();
        let (mut app, handle) = start(dir.path());

        // Nothing done yet: Esc dismisses at once.
        let untouched = ask(
            &mut app,
            vec![question("Q1?", K::Text, &[]), question("Q2?", K::Text, &[])],
        );
        press(&mut app, &handle, KeyCode::Esc);
        assert!(app.modal.is_none());
        assert!(untouched.await.is_err(), "the sender was dropped");

        let answers = ask(
            &mut app,
            vec![question("Q1?", K::Text, &[]), question("Q2?", K::Text, &[])],
        );
        type_text(&mut app, &handle, "one");
        press(&mut app, &handle, KeyCode::Enter);
        press(&mut app, &handle, KeyCode::Esc);
        let s = screen(&app);
        assert!(
            s.contains("Dismiss all 2 questions?") && s.contains("y dismiss all"),
            "{s}"
        );
        type_text(&mut app, &handle, "n");
        assert!(app.modal.is_some(), "n keeps answering");
        assert!(!screen(&app).contains("Dismiss all"));
        assert!(
            matches!(&app.modal, Some(Modal::Question(q)) if q.input.is_empty()),
            "the n was not typed"
        );
        press(&mut app, &handle, KeyCode::Esc);
        type_text(&mut app, &handle, "y");
        assert!(app.modal.is_none());
        assert!(answers.await.is_err(), "the sender was dropped");
    }

    #[tokio::test]
    async fn plain_messages_get_no_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, handle) = start(dir.path());
        type_text(&mut app, &handle, "hello /mode");
        assert!(app.suggestions().is_empty());
    }
}

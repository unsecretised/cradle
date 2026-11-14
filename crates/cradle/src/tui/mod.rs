//! The terminal UI: a transcript, an input box, and modals for approvals and questions.

mod commands;
mod markdown;
mod onboard;
mod picker;
mod text;
mod view;

use std::io::stdout;
use std::time::Duration;

use cradle_backends::Usage;
use cradle_core::{AgentCommand, AgentEvent, AgentHandle, Approval, ReviewPolicy, Verdict};
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyEventKind, KeyModifiers,
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
use text::Input;

pub async fn run(session: Session) -> Result<(), Error> {
    let mut terminal = ratatui::init();
    let _ = execute!(stdout(), EnableBracketedPaste);
    let result = start(&mut terminal, session).await;
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
    Tool {
        id: String,
        summary: String,
        state: ToolState,
        preview: String,
    },
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

#[derive(Clone, Copy, PartialEq, Eq)]
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
        allow_always: bool,
        reply: Option<oneshot::Sender<Approval>>,
        scroll: usize,
    },
    Question {
        question: String,
        options: Vec<String>,
        selected: usize,
        input: Input,
        reply: Option<oneshot::Sender<String>>,
    },
    Help,
    Model(ModelPicker),
}

pub(crate) struct App {
    pub items: Vec<Item>,
    /// Lines scrolled up from the bottom of the transcript.
    pub scroll: usize,
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
    ui: mpsc::UnboundedSender<UiEvent>,
    ui_events: Option<mpsc::UnboundedReceiver<UiEvent>>,
    /// Mode names and whether each is read-only, for suggestions.
    pub modes: Vec<(String, bool)>,
    /// Models offered as `/model` suggestions (configured, then any listed live).
    known_models: Vec<String>,
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
            .map(|p| match &p.backend {
                SpecBackend::Http(h) => h.models.clone(),
                SpecBackend::Cli(c) => c.models.clone(),
            })
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
            items.push(Item::Notice(format!(
                "Resumed a conversation with {} messages.",
                info.resumed
            )));
        }
        App {
            items,
            scroll: 0,
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
            ui,
            ui_events: Some(ui_events),
            modes,
            known_models,
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
            AgentEvent::ToolStarted { id, summary } => self.items.push(Item::Tool {
                id,
                summary,
                state: ToolState::Running,
                preview: String::new(),
            }),
            AgentEvent::ToolFinished { id, ok, preview } => {
                for item in self.items.iter_mut().rev() {
                    if let Item::Tool {
                        id: item_id,
                        state,
                        preview: p,
                        ..
                    } = item
                        && *item_id == id
                    {
                        *state = if ok { ToolState::Ok } else { ToolState::Failed };
                        *p = preview;
                        break;
                    }
                }
            }
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
                    allow_always: req.allow_always,
                    reply: Some(req.reply),
                    scroll: 0,
                })
            }
            AgentEvent::Question(req) => {
                self.modal = Some(Modal::Question {
                    question: req.question,
                    options: req.options,
                    selected: 0,
                    input: Input::default(),
                    reply: Some(req.reply),
                })
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
            AgentEvent::ReviewChanged(policy) => self.review = policy,
            AgentEvent::ModelChanged { provider, model } => {
                self.items
                    .push(Item::Notice(format!("Model: {provider}/{model}")));
                self.provider = provider;
                self.model = model;
            }
            AgentEvent::Usage(usage) => self.usage = usage,
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
            Event::Paste(text) => match &mut self.modal {
                Some(Modal::Question { input, .. }) => input.insert_str(&text.replace('\r', "")),
                Some(Modal::Model(picker)) => picker.insert_str(&text),
                Some(_) => {}
                None => self.input.insert_str(&text.replace('\r', "")),
            },
            _ => {}
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
        match key.code {
            KeyCode::Enter
                if key
                    .modifiers
                    .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
            {
                self.input.insert('\n')
            }
            KeyCode::Enter => self.submit(handle),
            KeyCode::Esc if self.busy => handle.cancel(),
            KeyCode::BackTab => {
                if self.busy {
                    self.note("Wait for the turn to finish before switching modes.");
                } else {
                    send(handle, AgentCommand::CycleMode);
                }
            }
            KeyCode::Char(c) => self.input.insert(c),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Up if !self.input.text().contains('\n') => self.recall(-1),
            KeyCode::Down if !self.input.text().contains('\n') => self.recall(1),
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            _ => {}
        }
        if self.input.text() != before {
            self.suggest_index = 0;
            self.suggest_hidden = false;
        }
    }

    /// Suggestions for the current input (empty when hidden or not a slash command).
    pub fn suggestions(&self) -> Vec<commands::Suggestion> {
        if self.suggest_hidden || self.modal.is_some() {
            return Vec::new();
        }
        let ctx = commands::Context {
            modes: &self.modes,
            current_mode: &self.mode,
            models: &self.known_models,
            current_model: &self.model,
            is_cli: self.is_cli(),
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
            Modal::Model(picker) => match picker.on_key(key.code) {
                PickerAction::Cancel => self.modal = None,
                PickerAction::Chosen(model) => {
                    self.modal = None;
                    self.switch_model(model, handle);
                }
                PickerAction::None => {}
            },
            Modal::Help => {
                if matches!(key.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.modal = None;
                }
            }
            Modal::Approval {
                allow_always,
                reply,
                scroll,
                ..
            } => {
                let decision = match key.code {
                    KeyCode::Char('y') => Some(Approval::Yes),
                    KeyCode::Char('a') if *allow_always => Some(Approval::Always),
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
            Modal::Question {
                options,
                selected,
                input,
                reply,
                ..
            } => {
                let answer = match key.code {
                    KeyCode::Esc => {
                        // Dropping the sender tells the agent the question was dismissed.
                        self.modal = None;
                        return;
                    }
                    KeyCode::Up => {
                        *selected = selected.saturating_sub(1);
                        None
                    }
                    KeyCode::Down if !options.is_empty() => {
                        *selected = (*selected + 1).min(options.len() - 1);
                        None
                    }
                    KeyCode::Enter if !input.is_empty() => Some(input.take()),
                    KeyCode::Enter => options.get(*selected).cloned(),
                    KeyCode::Char(c @ '1'..='9') if input.is_empty() => {
                        options.get(c as usize - '1' as usize).cloned()
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
                if let Some(answer) = answer {
                    if let Some(reply) = reply.take() {
                        let _ = reply.send(answer);
                    }
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
        let idle_only = matches!(name, "mode" | "plan" | "build" | "clear" | "diff" | "model");
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
            ("model", Some("default")) if self.is_cli() => self.switch_model(None, handle),
            ("model", Some(model)) => self.switch_model(Some(model.to_owned()), handle),
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
                for model in &models {
                    if !self.known_models.contains(model) {
                        self.known_models.push(model.clone());
                    }
                }
                if let Some(Modal::Model(picker)) = &mut self.modal {
                    picker.set_models(models, error);
                }
            }
        }
    }

    fn spec(&self) -> Option<&ProviderSpec> {
        self.providers.iter().find(|p| p.id == self.provider)
    }

    pub fn is_cli(&self) -> bool {
        self.spec()
            .is_some_and(|s| matches!(s.backend, SpecBackend::Cli(_)))
    }

    /// Open the picker and list the provider's models in the background.
    fn open_model_picker(&mut self) {
        let Some(spec) = self.spec().cloned() else {
            self.note("The current provider is not in the provider list.");
            return;
        };
        let is_cli = matches!(spec.backend, SpecBackend::Cli(_));
        let current = (!(is_cli && self.model == "default")).then_some(self.model.as_str());
        self.modal = Some(Modal::Model(ModelPicker::new(is_cli, current)));
        let ui = self.ui.clone();
        tokio::spawn(async move {
            let (models, error) = available_models(&spec).await;
            let _ = ui.send(UiEvent::Models(models, error.map(|e| e.to_string())));
        });
    }

    /// Switch to `model` on the current provider and save it as the default.
    fn switch_model(&mut self, model: Option<String>, handle: &AgentHandle) {
        let Some(spec) = self.spec().cloned() else {
            self.note("The current provider is not in the provider list.");
            return;
        };
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
        assert!(s.contains("small") && s.contains("large"), "{s}");
        press(&mut app, &handle, KeyCode::Esc);
        assert!(app.suggestions().is_empty(), "Esc hides the popup");
        assert!(!screen(&app).contains("↑↓ select"));
    }

    #[tokio::test]
    async fn plain_messages_get_no_suggestions() {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, handle) = start(dir.path());
        type_text(&mut app, &handle, "hello /mode");
        assert!(app.suggestions().is_empty());
    }
}

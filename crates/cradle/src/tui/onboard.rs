//! The setup screen: choose a provider, enter an API key if needed (saved to the OS
//! keychain), choose a model. Shown on first run, when the saved provider is unusable,
//! and with `--setup`.

use std::path::Path;

use cradle_backends::cli::installed_version;
use cradle_backends::credentials::{self, KeySource};
use cradle_backends::{Backend, available_models};
use cradle_config::user::UserConfig;
use cradle_provider_spec::{Auth, Backend as SpecBackend, CliAgent, HttpApi, ProviderSpec};
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Padding, Paragraph};

use super::picker::{ModelPicker, PickerAction};
use super::text::{Input, wrap};
use crate::Error;

enum Status {
    Ready(String),
    NeedsKey(String),
    CliInstalled(String),
    CliMissing,
}

struct Row {
    title: String,
    status: Status,
}

enum Step {
    Pick,
    Key { input: Input },
    Model { picker: ModelPicker },
}

struct Screen<'a> {
    providers: &'a [ProviderSpec],
    rows: Vec<Row>,
    reason: String,
    selected: usize,
    step: Step,
    message: Option<(String, bool)>,
    /// The model step was just opened and its list must be fetched.
    needs_models: bool,
}

/// Run the setup screen. Returns `None` if the user quits.
pub async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    keys: &mut EventStream,
    providers: &[ProviderSpec],
    reason: String,
    preselect: Option<&str>,
    user_config: Option<&Path>,
) -> Result<Option<Backend>, Error> {
    let rows = providers.iter().map(row).collect();
    let selected = preselect
        .and_then(|id| providers.iter().position(|p| p.id == id))
        .unwrap_or(0);
    let mut screen = Screen {
        providers,
        rows,
        reason,
        selected,
        step: Step::Pick,
        message: None,
        needs_models: false,
    };

    loop {
        terminal
            .draw(|frame| screen.draw(frame))
            .map_err(Error::Terminal)?;
        let event = match keys.next().await {
            Some(Ok(event)) => event,
            Some(Err(e)) => return Err(Error::Terminal(e)),
            None => return Ok(None),
        };
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('d'))
                {
                    return Ok(None);
                }
                if let Some(outcome) = screen.on_key(key.code, user_config) {
                    return Ok(outcome);
                }
            }
            Event::Paste(text) => match &mut screen.step {
                Step::Key { input } => input.insert_str(text.trim()),
                Step::Model { picker } => picker.insert_str(&text),
                Step::Pick => {}
            },
            _ => {}
        }

        if screen.needs_models {
            screen.needs_models = false;
            terminal
                .draw(|frame| screen.draw(frame))
                .map_err(Error::Terminal)?;
            let (models, error) = available_models(&providers[screen.selected]).await;
            if let Step::Model { picker } = &mut screen.step {
                picker.set_models(models, error.map(|e| e.to_string()));
            }
        }
    }
}

impl Screen<'_> {
    /// Handle a key. `Some(None)` quits, `Some(Some(b))` finishes with a backend.
    fn on_key(&mut self, code: KeyCode, user_config: Option<&Path>) -> Option<Option<Backend>> {
        match &mut self.step {
            Step::Pick => match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.selected = self.selected.saturating_sub(1);
                    self.message = None;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1).min(self.rows.len().saturating_sub(1));
                    self.message = None;
                }
                KeyCode::Enter => return self.choose(),
                KeyCode::Esc | KeyCode::Char('q') => return Some(None),
                _ => {}
            },
            Step::Key { input } => match code {
                KeyCode::Esc => {
                    self.step = Step::Pick;
                    self.message = None;
                }
                KeyCode::Enter => {
                    let key = input.text().trim().to_owned();
                    let Status::NeedsKey(env) = &self.rows[self.selected].status else {
                        self.step = Step::Pick;
                        return None;
                    };
                    if key.is_empty() || key.contains(char::is_whitespace) {
                        self.message = Some(("That doesn't look like an API key.".into(), true));
                        return None;
                    }
                    let env = env.clone();
                    match credentials::save_key(&env, &key) {
                        Ok(()) => {
                            self.rows[self.selected].status =
                                Status::Ready(format!("key in {}", credentials::store_name()));
                            self.message = Some((
                                format!("Saved to the {}.", credentials::store_name()),
                                false,
                            ));
                            return self.choose_model();
                        }
                        Err(e) => {
                            self.message = Some((
                                format!("{e}. You can set ${env} instead and restart."),
                                true,
                            ));
                        }
                    }
                }
                KeyCode::Backspace => input.backspace(),
                KeyCode::Char(c) => input.insert(c),
                _ => {}
            },
            Step::Model { picker } => match picker.on_key(code) {
                PickerAction::Cancel => {
                    self.step = Step::Pick;
                    self.message = None;
                }
                PickerAction::Chosen(model) => return self.finish(model.as_deref(), user_config),
                PickerAction::None => {}
            },
        }
        None
    }

    fn choose(&mut self) -> Option<Option<Backend>> {
        self.message = None;
        match &self.rows[self.selected].status {
            Status::NeedsKey(_) => {
                self.step = Step::Key {
                    input: Input::default(),
                };
                None
            }
            Status::Ready(_) => self.choose_model(),
            Status::CliInstalled(_) => self.choose_model(),
            Status::CliMissing => {
                self.message = Some(("Not installed on this machine.".into(), true));
                None
            }
        }
    }

    fn choose_model(&mut self) -> Option<Option<Backend>> {
        let spec = &self.providers[self.selected];
        let default = match &spec.backend {
            SpecBackend::Http(h) => h.default_model.as_deref(),
            SpecBackend::Cli(c) => c.default_model.as_deref(),
        };
        self.step = Step::Model {
            picker: ModelPicker::new(self.is_cli(), default),
        };
        self.needs_models = true;
        None
    }

    fn finish(
        &mut self,
        model: Option<&str>,
        user_config: Option<&Path>,
    ) -> Option<Option<Backend>> {
        let spec = &self.providers[self.selected];
        match Backend::new(spec, model) {
            Ok(backend) => {
                if let Some(path) = user_config {
                    let mut config = UserConfig::load(path).unwrap_or_default();
                    config.default.provider = Some(spec.id.clone());
                    config.default.model = model.map(str::to_owned);
                    // Not fatal: the choice still applies to this session.
                    let _ = config.save(path);
                }
                Some(Some(backend))
            }
            Err(e) => {
                self.step = Step::Pick;
                self.message = Some((e.to_string(), true));
                None
            }
        }
    }

    fn is_cli(&self) -> bool {
        matches!(self.providers[self.selected].backend, SpecBackend::Cli(_))
    }

    fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        let width = area.width.saturating_sub(4).min(96);
        let rect = Rect {
            x: area.x + area.width.saturating_sub(width) / 2,
            y: area.y + 1.min(area.height),
            width,
            height: area.height.saturating_sub(2),
        };
        let inner = width.saturating_sub(4) as usize;
        let mut lines: Vec<Line> = Vec::new();

        match &self.step {
            Step::Pick => {
                let intro = if self.reason.is_empty() {
                    "Welcome to Cradle. Choose how to connect to a model. You can change this \
                     later with `cradle --setup`."
                        .to_owned()
                } else {
                    self.reason.clone()
                };
                let style = if self.reason.is_empty() {
                    Style::new()
                } else {
                    Style::new().fg(Color::Yellow)
                };
                for l in wrap(&intro, inner) {
                    lines.push(Line::styled(l, style));
                }
                lines.push(Line::raw(""));
                for (i, row) in self.rows.iter().enumerate() {
                    let marker = if i == self.selected { "▸ " } else { "  " };
                    let title_style = if i == self.selected {
                        Style::new().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                    } else {
                        Style::new().add_modifier(Modifier::BOLD)
                    };
                    let (status, color) = match &row.status {
                        Status::Ready(s) => (s.clone(), Color::Green),
                        Status::NeedsKey(env) => {
                            (format!("needs an API key (${env})"), Color::Yellow)
                        }
                        Status::CliInstalled(v) => (format!("installed ({v})"), Color::Green),
                        Status::CliMissing => ("not installed".into(), Color::DarkGray),
                    };
                    lines.push(Line::from(vec![
                        Span::raw(marker),
                        Span::styled(format!(" {} ", row.title), title_style),
                        Span::raw("  "),
                        Span::styled(status, Style::new().fg(color)),
                    ]));
                    lines.push(Line::styled(
                        format!("    {}", self.providers[i].id),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
            }
            Step::Key { input } => {
                let row = &self.rows[self.selected];
                let env = match &row.status {
                    Status::NeedsKey(env) => env.as_str(),
                    _ => "",
                };
                let text = format!(
                    "Paste your API key for {}.\n\nIt is saved in the {} (service \"cradle\", \
                     account {env}) and never written to a file. You can also set ${env} in \
                     your shell instead.",
                    row.title,
                    credentials::store_name()
                );
                for l in wrap(&text, inner) {
                    lines.push(Line::raw(l));
                }
                lines.push(Line::raw(""));
                let masked: String = "•".repeat(input.text().chars().count());
                lines.push(Line::from(vec![
                    Span::styled("Key: ", Style::new().fg(Color::DarkGray)),
                    Span::raw(masked),
                    Span::styled("▏", Style::new().fg(Color::Cyan)),
                ]));
            }
            Step::Model { picker } => {
                lines.push(Line::styled(
                    format!("Choose a model for {}.", self.rows[self.selected].title),
                    Style::new().add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::raw(""));
                let rows = (rect.height as usize).saturating_sub(10);
                lines.extend(picker.lines(inner, rows));
            }
        }

        if let Some((message, is_error)) = &self.message {
            lines.push(Line::raw(""));
            let color = if *is_error { Color::Red } else { Color::Green };
            for l in wrap(message, inner) {
                lines.push(Line::styled(l, Style::new().fg(color)));
            }
        }

        let keys = match self.step {
            Step::Pick => " ↑↓ choose · Enter select · Esc quit ",
            Step::Key { .. } => " paste or type · Enter save · Esc back ",
            Step::Model { .. } => " type to filter · ↑↓ choose · Enter confirm · Esc back ",
        };
        let block = Block::bordered()
            .title(Line::styled(
                " Set up Cradle ",
                Style::new().add_modifier(Modifier::BOLD),
            ))
            .title_bottom(keys)
            .border_style(Style::new().fg(Color::Cyan))
            .padding(Padding::new(1, 1, 1, 0));
        frame.render_widget(Paragraph::new(lines).block(block), rect);
    }
}

fn row(spec: &ProviderSpec) -> Row {
    match &spec.backend {
        SpecBackend::Http(http) => {
            let title = match http.api {
                HttpApi::Anthropic => "Anthropic API".to_owned(),
                HttpApi::OpenaiCompatible if spec.id == "ollama" => "Ollama (local)".to_owned(),
                HttpApi::OpenaiCompatible => format!("{} (OpenAI-compatible)", spec.id),
            };
            let status = match &http.auth {
                Auth::None => Status::Ready("local, no key needed".into()),
                Auth::ApiKey { env } => match credentials::find_key(env) {
                    Some((_, KeySource::Env)) => Status::Ready(format!("key from ${env}")),
                    Some((_, KeySource::Keychain)) => {
                        Status::Ready(format!("key in {}", credentials::store_name()))
                    }
                    None => Status::NeedsKey(env.clone()),
                },
            };
            Row { title, status }
        }
        SpecBackend::Cli(cli) => {
            let title = match cli.agent {
                CliAgent::ClaudeCode => "Claude Code (your Claude subscription)",
                CliAgent::Codex => "Codex (your ChatGPT plan)",
            };
            let status = match installed_version(&cli.binary) {
                Some(version) => Status::CliInstalled(version),
                None => Status::CliMissing,
            };
            Row {
                title: title.to_owned(),
                status,
            }
        }
    }
}

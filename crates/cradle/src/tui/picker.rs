//! The model picker, shared by the setup screen and `/model`. Typing filters the list;
//! anything typed can also be used as a model name as-is.

use crossterm::event::KeyCode;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::text::{Input, wrap};
use super::theme::Theme;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    /// The CLI's own default model (no `--model`).
    Default,
    Model(String),
    /// Exactly what the user typed.
    Custom(String),
}

pub enum PickerAction {
    None,
    /// `None` means the CLI's default model.
    Chosen(Option<String>),
    Cancel,
}

pub struct ModelPicker {
    models: Vec<String>,
    allow_default: bool,
    current: Option<String>,
    selected: usize,
    input: Input,
    loading: bool,
    note: Option<String>,
}

impl ModelPicker {
    /// A picker waiting for its list. `allow_default` offers "the CLI's default".
    pub fn new(allow_default: bool, current: Option<&str>) -> Self {
        ModelPicker {
            models: Vec::new(),
            allow_default,
            current: current.map(str::to_owned),
            selected: 0,
            input: Input::default(),
            loading: true,
            note: None,
        }
    }

    /// Fill in the list. `error` explains why only the configured models are shown.
    pub fn set_models(&mut self, models: Vec<String>, error: Option<String>) {
        self.models = models;
        self.loading = false;
        self.note =
            error.map(|e| format!("Couldn't list models live ({e}); showing configured ones."));
        let entries = self.entries();
        self.selected = entries
            .iter()
            .position(|e| match (e, &self.current) {
                (Entry::Model(m), Some(current)) => m == current,
                (Entry::Default, None) => true,
                _ => false,
            })
            .unwrap_or(0);
    }

    pub fn insert_str(&mut self, text: &str) {
        self.input.insert_str(text.trim());
        self.selected = 0;
    }

    pub fn on_key(&mut self, code: KeyCode) -> PickerAction {
        let count = self.entries().len();
        match code {
            KeyCode::Esc => return PickerAction::Cancel,
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down => self.selected = (self.selected + 1).min(count.saturating_sub(1)),
            KeyCode::PageUp => self.selected = self.selected.saturating_sub(10),
            KeyCode::PageDown => self.selected = (self.selected + 10).min(count.saturating_sub(1)),
            // While the list loads, a typed name can still be used as-is.
            KeyCode::Enter if self.loading => {
                let typed = self.input.text().trim();
                if !typed.is_empty() {
                    return PickerAction::Chosen(Some(typed.to_owned()));
                }
            }
            KeyCode::Enter => {
                return match self.entries().get(self.selected) {
                    Some(Entry::Default) => PickerAction::Chosen(None),
                    Some(Entry::Model(m) | Entry::Custom(m)) => {
                        PickerAction::Chosen(Some(m.clone()))
                    }
                    None => PickerAction::None,
                };
            }
            KeyCode::Backspace => {
                self.input.backspace();
                self.selected = 0;
            }
            KeyCode::Char(c) => {
                self.input.insert(c);
                self.selected = 0;
            }
            _ => {}
        }
        PickerAction::None
    }

    /// The rows to show: `rows` limits the list's height (it scrolls to the selection).
    pub fn lines(&self, width: usize, rows: usize, theme: &Theme) -> Vec<Line<'static>> {
        let dim = theme.hint;
        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled("Filter or type a model: ", dim),
            Span::raw(self.input.text().to_owned()),
            Span::styled("▏", theme.cursor),
        ]));
        lines.push(Line::raw(""));
        if self.loading {
            lines.push(Line::styled("Loading models…", dim));
            return lines;
        }

        let entries = self.entries();
        let rows = rows.max(3);
        let first = self
            .selected
            .saturating_sub(rows - 1)
            .min(entries.len().saturating_sub(rows));
        if first > 0 {
            lines.push(Line::styled(format!("  ↑ {first} more"), dim));
        }
        for (i, entry) in entries.iter().enumerate().skip(first).take(rows) {
            let label = match entry {
                Entry::Default => "default (the CLI chooses)".to_owned(),
                Entry::Model(m) => m.clone(),
                Entry::Custom(m) => format!("use “{m}”"),
            };
            let is_current = match (entry, &self.current) {
                (Entry::Model(m), Some(c)) => m == c,
                (Entry::Default, None) => !self.loading,
                _ => false,
            };
            let mark = if is_current { " (current)" } else { "" };
            let style = if i == self.selected {
                theme.selection
            } else if matches!(entry, Entry::Custom(_)) {
                Style::new().add_modifier(Modifier::ITALIC)
            } else {
                Style::new()
            };
            lines.push(Line::styled(format!(" {label}{mark} "), style));
        }
        let below = entries.len().saturating_sub(first + rows);
        if below > 0 {
            lines.push(Line::styled(format!("  ↓ {below} more"), dim));
        }
        if let Some(note) = &self.note {
            lines.push(Line::raw(""));
            for l in wrap(note, width) {
                lines.push(Line::styled(l, theme.note));
            }
        }
        lines
    }

    fn entries(&self) -> Vec<Entry> {
        let filter = self.input.text().trim().to_lowercase();
        let mut entries = Vec::new();
        if self.allow_default && filter.is_empty() {
            entries.push(Entry::Default);
        }
        entries.extend(
            self.models
                .iter()
                .filter(|m| m.to_lowercase().contains(&filter))
                .map(|m| Entry::Model(m.clone())),
        );
        let typed = self.input.text().trim();
        if !typed.is_empty() && !self.models.iter().any(|m| m == typed) {
            entries.push(Entry::Custom(typed.to_owned()));
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(allow_default: bool, current: Option<&str>) -> ModelPicker {
        let mut p = ModelPicker::new(allow_default, current);
        p.set_models(vec!["opus".into(), "sonnet".into(), "haiku".into()], None);
        p
    }

    #[test]
    fn selects_current_and_chooses() {
        let mut p = picker(false, Some("sonnet"));
        assert!(matches!(p.on_key(KeyCode::Enter), PickerAction::Chosen(Some(m)) if m == "sonnet"));
        p.on_key(KeyCode::Down);
        assert!(matches!(p.on_key(KeyCode::Enter), PickerAction::Chosen(Some(m)) if m == "haiku"));
    }

    #[test]
    fn default_entry_for_clis() {
        let mut p = picker(true, None);
        assert!(matches!(
            p.on_key(KeyCode::Enter),
            PickerAction::Chosen(None)
        ));
    }

    #[test]
    fn typing_filters_and_allows_custom_names() {
        let mut p = picker(true, None);
        p.insert_str("so");
        assert!(matches!(p.on_key(KeyCode::Enter), PickerAction::Chosen(Some(m)) if m == "sonnet"));
        p.insert_str("nnet-x");
        assert!(
            matches!(p.on_key(KeyCode::Enter), PickerAction::Chosen(Some(m)) if m == "sonnet-x"),
            "no match: the typed name itself is offered"
        );
    }

    #[test]
    fn nothing_chosen_while_loading() {
        let mut p = ModelPicker::new(false, None);
        assert!(matches!(p.on_key(KeyCode::Enter), PickerAction::None));
    }
}

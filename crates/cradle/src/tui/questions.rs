//! The `ask_user` modal: one or more questions, answered one at a time.
//!
//! Choice questions (`single`, `multi`) also take a typed answer of the user's own.
//! Shift+Tab goes back to an earlier question. Esc dismisses the whole batch, after a
//! confirmation once anything has been answered or typed.

use cradle_core::Answer;
use cradle_tools::{Question, QuestionKind};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use tokio::sync::oneshot;

use super::text::{Input, display_width, wrap};
use super::theme::Theme;

pub(crate) struct Questions {
    questions: Vec<Question>,
    answers: Vec<Option<Answer>>,
    /// The question shown now.
    current: usize,
    /// The highlighted option.
    selected: usize,
    /// Ticked options, for a multi-choice question.
    checked: Vec<bool>,
    pub input: Input,
    /// Asking whether to dismiss every question.
    confirm_dismiss: bool,
    reply: Option<oneshot::Sender<Vec<Answer>>>,
}

impl Questions {
    pub fn new(questions: Vec<Question>, reply: oneshot::Sender<Vec<Answer>>) -> Self {
        let mut q = Questions {
            answers: vec![None; questions.len()],
            questions,
            current: 0,
            selected: 0,
            checked: Vec::new(),
            input: Input::default(),
            confirm_dismiss: false,
            reply: Some(reply),
        };
        q.load();
        q
    }

    fn question(&self) -> &Question {
        &self.questions[self.current]
    }

    /// Show the current question, with its earlier answer if it has one.
    fn load(&mut self) {
        let options = self.questions[self.current].options.clone();
        self.selected = 0;
        self.checked = vec![false; options.len()];
        self.input = Input::default();
        match &self.answers[self.current] {
            Some(Answer::Choice(c)) => {
                self.selected = options.iter().position(|o| o == c).unwrap_or(0)
            }
            Some(Answer::Choices { picked, other }) => {
                for (i, option) in options.iter().enumerate() {
                    self.checked[i] = picked.contains(option);
                }
                if let Some(other) = other {
                    self.input.set(other.clone());
                }
            }
            Some(Answer::Text(t)) => self.input.set(t.clone()),
            None => {}
        }
    }

    /// The answer as the current question stands, if it can be sent.
    fn current_answer(&self) -> Option<Answer> {
        let q = self.question();
        let typed = self.input.text().trim();
        let typed = (!typed.is_empty()).then(|| typed.to_owned());
        match q.kind {
            QuestionKind::Text => typed.map(Answer::Text),
            QuestionKind::Single => match typed {
                Some(t) => Some(Answer::Text(t)),
                None => q.options.get(self.selected).cloned().map(Answer::Choice),
            },
            QuestionKind::Multi => {
                let picked: Vec<String> = q
                    .options
                    .iter()
                    .zip(&self.checked)
                    .filter(|(_, c)| **c)
                    .map(|(o, _)| o.clone())
                    .collect();
                (!picked.is_empty() || typed.is_some()).then_some(Answer::Choices {
                    picked,
                    other: typed,
                })
            }
        }
    }

    /// Whether dismissing would throw away anything the user did.
    fn started(&self) -> bool {
        self.answers.iter().any(Option::is_some)
            || !self.input.is_empty()
            || self.checked.iter().any(|c| *c)
    }

    /// Record `answer` and move on. Returns `true` once every question is answered (the
    /// answers have then been sent).
    fn answer(&mut self, answer: Answer) -> bool {
        self.answers[self.current] = Some(answer);
        if self.current + 1 < self.questions.len() {
            self.current += 1;
            self.load();
            return false;
        }
        let answers = self.answers.drain(..).flatten().collect();
        if let Some(reply) = self.reply.take() {
            let _ = reply.send(answers);
        }
        true
    }

    /// Handle a key. Returns `true` when the modal should close: either every answer was
    /// sent, or the user dismissed the questions (the sender is dropped with the modal).
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        if self.confirm_dismiss {
            match key.code {
                KeyCode::Char('y' | 'Y') => return true,
                KeyCode::Char('n' | 'N') | KeyCode::Esc => self.confirm_dismiss = false,
                _ => {}
            }
            return false;
        }
        let kind = self.question().kind;
        let count = self.question().options.len();
        match key.code {
            KeyCode::Esc if self.started() => self.confirm_dismiss = true,
            KeyCode::Esc => return true,
            KeyCode::BackTab if self.current > 0 => {
                // Keep what was entered here, if it is a full answer.
                if let Some(answer) = self.current_answer() {
                    self.answers[self.current] = Some(answer);
                }
                self.current -= 1;
                self.load();
            }
            KeyCode::Enter => {
                if let Some(answer) = self.current_answer() {
                    return self.answer(answer);
                }
            }
            KeyCode::Up => self.selected = self.selected.saturating_sub(1),
            KeyCode::Down if count > 0 => self.selected = (self.selected + 1).min(count - 1),
            KeyCode::Char(' ') if kind == QuestionKind::Multi && self.input.is_empty() => {
                if let Some(c) = self.checked.get_mut(self.selected) {
                    *c = !*c;
                }
            }
            KeyCode::Char(c @ '0'..='9') if kind != QuestionKind::Text && self.input.is_empty() => {
                let index = if c == '0' {
                    9
                } else {
                    c as usize - '1' as usize
                };
                if index < count {
                    self.selected = index;
                    if kind == QuestionKind::Single {
                        let choice = self.question().options[index].clone();
                        return self.answer(Answer::Choice(choice));
                    }
                    self.checked[index] = !self.checked[index];
                }
            }
            KeyCode::Char(c) => self.input.insert(c),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            _ => {}
        }
        false
    }

    pub fn title(&self) -> String {
        match self.questions.len() {
            1 => " The agent asks ".to_owned(),
            n => format!(" The agent asks ({} of {n}) ", self.current + 1),
        }
    }

    pub fn hint(&self) -> String {
        if self.confirm_dismiss {
            return " y dismiss all · n keep answering ".to_owned();
        }
        let q = self.question();
        let enter = if self.current + 1 < self.questions.len() {
            "Enter next"
        } else {
            "Enter send"
        };
        let back = if self.current > 0 {
            " · ⇧Tab back"
        } else {
            ""
        };
        let keys = match q.options.len() {
            10 => "1-9,0",
            _ => "1-9",
        };
        let how = match q.kind {
            QuestionKind::Single => format!("↑↓ select · {keys} pick · type your own"),
            QuestionKind::Multi => format!("↑↓ move · Space/{keys} toggle · type your own"),
            QuestionKind::Text => "type your answer".to_owned(),
        };
        format!(" {how} · {enter}{back} · Esc dismiss ")
    }

    /// The modal body, at most `rows` lines where possible. Earlier answers are shown
    /// above the current question, the oldest dropped first when space runs out.
    pub fn lines(&self, width: usize, rows: usize, theme: &Theme) -> Vec<Line<'static>> {
        let dim = theme.hint;
        let q = self.question();
        let mut body: Vec<Line<'static>> = Vec::new();
        for l in wrap(&q.question, width) {
            body.push(Line::styled(l, theme.title));
        }
        body.push(Line::raw(""));
        for (i, option) in q.options.iter().enumerate() {
            let key = if i == 9 { 0 } else { i + 1 };
            let tick = match (q.kind, self.checked.get(i)) {
                (QuestionKind::Multi, Some(true)) => "[x] ",
                (QuestionKind::Multi, _) => "[ ] ",
                _ => "",
            };
            let style = if i == self.selected && self.input.is_empty() {
                theme.selection
            } else {
                Style::new()
            };
            body.push(Line::styled(format!(" {key}. {tick}{option} "), style));
        }
        if !q.options.is_empty() {
            body.push(Line::raw(""));
        }
        let label = match q.kind {
            QuestionKind::Text => "Answer: ",
            _ => "Or your own: ",
        };
        body.push(Line::from(vec![
            Span::styled(label, dim),
            Span::raw(self.input.text().replace('\n', " ")),
            Span::styled("▏", theme.cursor),
        ]));
        if self.confirm_dismiss {
            body.push(Line::raw(""));
            let n = self.questions.len();
            let what = if n == 1 {
                "this question".to_owned()
            } else {
                format!("all {n} questions")
            };
            body.push(Line::styled(
                format!(
                    "Dismiss {what}? Answers so far will be lost. [y] dismiss · [n] keep answering"
                ),
                theme.confirm,
            ));
        }

        let earlier: Vec<Line<'static>> = self.questions[..self.current]
            .iter()
            .zip(&self.answers)
            .filter_map(|(q, a)| {
                let a = a.as_ref()?;
                Some(Line::styled(
                    truncate(&format!("✓ {}: {}", q.question, summary(a)), width),
                    dim,
                ))
            })
            .collect();
        // Room for earlier answers plus a blank line before the question.
        let room = rows.saturating_sub(body.len() + 1);
        let mut lines = Vec::new();
        if !earlier.is_empty() && room > 0 {
            if earlier.len() > room {
                let shown = room - 1;
                let hidden = earlier.len() - shown;
                lines.push(Line::styled(format!("… {hidden} earlier"), dim));
                lines.extend(earlier.into_iter().skip(hidden));
            } else {
                lines.extend(earlier);
            }
            lines.push(Line::raw(""));
        }
        lines.extend(body);
        lines
    }
}

/// A one-line description of an answer.
fn summary(answer: &Answer) -> String {
    match answer {
        Answer::Choice(c) | Answer::Text(c) => c.replace('\n', " "),
        Answer::Choices { picked, other } => {
            let mut parts = picked.clone();
            if let Some(other) = other {
                parts.push(other.replace('\n', " "));
            }
            parts.join(", ")
        }
    }
}

/// Cut `s` to `width` columns, ending in `…` if anything was cut.
fn truncate(s: &str, width: usize) -> String {
    if display_width(s) <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    for c in s.chars() {
        if display_width(&out) + display_width(c.encode_utf8(&mut [0; 4])) + 1 > width {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

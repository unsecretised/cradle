//! Rendering.

use cradle_core::{ReviewPolicy, Verdict};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::commands::{COMMANDS, Suggestion};
use super::text::wrap;
use super::{App, Item, Modal, ToolState};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_INPUT_LINES: usize = 8;
const MAX_DIFF_LINES: usize = 40;
const MAX_THINKING_LINES: usize = 6;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let input_width = area.width.saturating_sub(2) as usize;
    let (input_lines, cursor) = app.input.layout(input_width);
    let input_height = input_lines.len().clamp(1, MAX_INPUT_LINES) as u16 + 2;

    let [header, body, input, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, header, app);
    draw_transcript(frame, body, app);
    draw_input(frame, input, app, &input_lines, cursor);
    frame.render_widget(
        Paragraph::new(
            " Enter send · Alt+Enter newline · / commands · Shift+Tab mode · Esc cancel · PgUp/PgDn scroll",
        )
        .style(dim()),
        footer,
    );

    if let Some(modal) = &app.modal {
        draw_modal(frame, area, modal);
    } else {
        let suggestions = app.suggestions();
        if !suggestions.is_empty() {
            draw_suggestions(frame, body, input, &suggestions, app.suggest_index);
        }
    }
}

const MAX_SUGGESTIONS: usize = 8;

/// The slash-command popup, just above the input box.
fn draw_suggestions(
    frame: &mut Frame,
    body: Rect,
    input: Rect,
    suggestions: &[Suggestion],
    selected: usize,
) {
    let selected = selected.min(suggestions.len() - 1);
    let rows = suggestions.len().min(MAX_SUGGESTIONS);
    let height = (rows as u16 + 2).min(body.height);
    if height < 3 {
        return;
    }
    let rect = Rect {
        x: input.x,
        y: input.y.saturating_sub(height),
        width: input.width,
        height,
    };
    let first = (selected + 1).saturating_sub(rows);
    let label_width = suggestions
        .iter()
        .map(|s| super::text::display_width(&s.label))
        .max()
        .unwrap_or(0)
        .min(rect.width as usize / 2);
    let lines: Vec<Line> = suggestions
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(i, s)| {
            let (label_style, detail_style) = if i == selected {
                let on = Style::new().fg(Color::Black).bg(Color::Cyan);
                (on.add_modifier(Modifier::BOLD), on)
            } else {
                (Style::new().add_modifier(Modifier::BOLD), dim())
            };
            let pad = label_width.saturating_sub(super::text::display_width(&s.label));
            Line::from(vec![
                Span::styled(format!(" {}{} ", s.label, " ".repeat(pad)), label_style),
                Span::styled(format!(" {} ", s.detail), detail_style),
            ])
        })
        .collect();
    let more = suggestions.len().saturating_sub(first + rows);
    let hint = if more > 0 {
        format!(" ↑↓ select · Tab complete · Enter run · {more} more ")
    } else {
        " ↑↓ select · Tab complete · Enter run · Esc hide ".to_owned()
    };
    let block = Block::bordered()
        .title_bottom(Line::styled(hint, dim()))
        .border_style(Style::new().fg(Color::Cyan));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let mode_style = if app.read_only {
        Style::new().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::new().fg(Color::Black).bg(Color::Green)
    };
    let mode = format!(
        " {}{} ",
        app.mode.to_uppercase(),
        if app.read_only { " · read-only" } else { "" }
    );
    let review = match app.review {
        ReviewPolicy::User => "commands: you approve",
        ReviewPolicy::Subagent => "commands: reviewer, then you",
    };
    let u = &app.usage;
    let line = Line::from(vec![
        Span::styled(
            " cradle ",
            Style::new().add_modifier(Modifier::REVERSED | Modifier::BOLD),
        ),
        Span::raw(format!(" {}/{} ", app.provider, app.model)),
        Span::styled(mode, mode_style.add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {review}"), dim()),
        Span::styled(
            format!(
                "  tokens in {} out {} cached {}",
                u.input_tokens, u.output_tokens, u.cache_read_tokens
            ),
            dim(),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &App) {
    let width = (area.width as usize).saturating_sub(2).max(10);
    let mut lines: Vec<Line> = Vec::new();
    for item in &app.items {
        render_item(item, width, app.spinner, &mut lines);
        lines.push(Line::raw(""));
    }
    if app.busy {
        let spin = SPINNER[app.spinner % SPINNER.len()];
        lines.push(Line::styled(
            format!("{spin} working…"),
            Style::new().fg(Color::Yellow),
        ));
    }

    let height = area.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    let scroll = app.scroll.min(max_scroll);
    let start = lines.len().saturating_sub(height + scroll);
    let end = (start + height).min(lines.len());
    let visible: Vec<Line> = lines.drain(start..end).collect();
    frame.render_widget(Paragraph::new(visible), area);
}

fn render_item(item: &Item, width: usize, spinner: usize, out: &mut Vec<Line<'static>>) {
    match item {
        Item::User(text) => gutter(
            out,
            "› ",
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            text,
            Style::new().add_modifier(Modifier::BOLD),
            width,
        ),
        Item::Assistant(text) => render_markdown(text, width, out),
        Item::Thinking(text) => {
            let wrapped = wrap(text.trim(), width);
            let skip = wrapped.len().saturating_sub(MAX_THINKING_LINES);
            let style = dim().add_modifier(Modifier::ITALIC);
            if skip > 0 {
                out.push(Line::styled(
                    format!("✻ … {skip} earlier lines of thinking"),
                    style,
                ));
            }
            for (i, line) in wrapped.into_iter().skip(skip).enumerate() {
                let mark = if i == 0 && skip == 0 { "✻ " } else { "  " };
                out.push(Line::styled(format!("{mark}{line}"), style));
            }
        }
        Item::Tool {
            summary,
            state,
            preview,
            ..
        } => {
            let (mark, color) = match state {
                ToolState::Running => (SPINNER[spinner % SPINNER.len()], Color::Yellow),
                ToolState::Ok => ("✓", Color::Green),
                ToolState::Failed => ("✗", Color::Red),
            };
            gutter(
                out,
                &format!("{mark} "),
                Style::new().fg(color),
                summary,
                Style::new(),
                width,
            );
            if !preview.is_empty() {
                let style = if *state == ToolState::Failed {
                    Style::new().fg(Color::Red)
                } else {
                    dim()
                };
                gutter(out, "  │ ", dim(), preview, style, width.saturating_sub(2));
            }
        }
        Item::Diff { title, diff } => {
            out.push(Line::styled(
                format!("  {title}"),
                dim().add_modifier(Modifier::BOLD),
            ));
            let lines: Vec<&str> = diff.lines().collect();
            for line in lines.iter().take(MAX_DIFF_LINES) {
                for part in wrap(line, width.saturating_sub(2)) {
                    out.push(Line::styled(format!("  {part}"), diff_style(line)));
                }
            }
            if lines.len() > MAX_DIFF_LINES {
                out.push(Line::styled(
                    format!(
                        "  … {} more lines (/diff shows all changes)",
                        lines.len() - MAX_DIFF_LINES
                    ),
                    dim(),
                ));
            }
        }
        Item::Review {
            command,
            verdict,
            reason,
        } => {
            let (word, color) = match verdict {
                Verdict::Safe => ("safe, running", Color::Green),
                Verdict::Unsafe => ("unsafe, asking you", Color::Red),
                Verdict::Unsure => ("unsure, asking you", Color::Yellow),
            };
            gutter(
                out,
                "◆ ",
                Style::new().fg(color),
                &format!("reviewer: `{command}` is {word}. {reason}"),
                Style::new().fg(color),
                width,
            );
        }
        Item::Notice(text) => gutter(out, "· ", dim(), text, dim(), width),
        Item::Error(text) => gutter(
            out,
            "! ",
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
            text,
            Style::new().fg(Color::Red),
            width,
        ),
    }
}

/// Model output, rendered as markdown with a two-column margin.
fn render_markdown(text: &str, width: usize, out: &mut Vec<Line<'static>>) {
    for line in super::markdown::render(text, width) {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(line.spans);
        out.push(Line::from(spans));
    }
}

/// Lines with a styled gutter on the first line and matching indentation after.
fn gutter(
    out: &mut Vec<Line<'static>>,
    mark: &str,
    mark_style: Style,
    text: &str,
    style: Style,
    width: usize,
) {
    let indent = " ".repeat(super::text::display_width(mark));
    for (i, line) in wrap(text, width).into_iter().enumerate() {
        let lead = if i == 0 {
            mark.to_owned()
        } else {
            indent.clone()
        };
        out.push(Line::from(vec![
            Span::styled(lead, mark_style),
            Span::styled(line, style),
        ]));
    }
}

fn draw_input(frame: &mut Frame, area: Rect, app: &App, lines: &[String], cursor: (usize, usize)) {
    let title = if app.busy {
        format!(
            " {} working · Esc to cancel ",
            SPINNER[app.spinner % SPINNER.len()]
        )
    } else {
        format!(" {} › ", app.mode)
    };
    let border = if app.read_only {
        Color::Cyan
    } else {
        Color::Green
    };
    let block = Block::bordered()
        .title(title)
        .border_style(Style::new().fg(border));
    let inner = block.inner(area);
    let visible = inner.height as usize;
    let first = (cursor.0 + 1).saturating_sub(visible);
    let text: Vec<Line> = lines
        .iter()
        .skip(first)
        .take(visible)
        .map(|l| Line::raw(l.clone()))
        .collect();
    frame.render_widget(Paragraph::new(text).block(block), area);
    if app.modal.is_none() {
        frame.set_cursor_position((
            inner.x + cursor.1 as u16,
            inner.y + (cursor.0 - first) as u16,
        ));
    }
}

fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal) {
    // Never larger than the terminal, however small it is.
    let width = area.width.saturating_sub(4).min(110);
    let height = area.height.saturating_sub(4);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let inner_width = width.saturating_sub(4) as usize;

    match modal {
        Modal::Approval {
            title,
            body,
            note,
            allow_always,
            scroll,
            ..
        } => {
            let mut lines: Vec<Line> = Vec::new();
            if let Some(note) = note {
                for l in wrap(note, inner_width) {
                    lines.push(Line::styled(l, Style::new().fg(Color::Yellow)));
                }
                lines.push(Line::raw(""));
            }
            for raw in body.lines() {
                for l in wrap(raw, inner_width) {
                    lines.push(Line::styled(l, diff_style(raw)));
                }
            }
            let keys = if *allow_always {
                " [y] yes · [a] always this session · [n] no · ↑↓ scroll "
            } else {
                " [y] yes · [n] no · ↑↓ scroll "
            };
            let block = Block::bordered()
                .title(format!(" {title} "))
                .title_bottom(Line::styled(
                    keys,
                    Style::new().add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::new().fg(Color::Yellow));
            let visible = rect.height.saturating_sub(2) as usize;
            let scroll = (*scroll).min(lines.len().saturating_sub(visible));
            let lines: Vec<Line> = lines.into_iter().skip(scroll).collect();
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block.padding(ratatui::widgets::Padding::horizontal(1))),
                rect,
            );
        }
        Modal::Question {
            question,
            options,
            selected,
            input,
            ..
        } => {
            let mut lines: Vec<Line> = Vec::new();
            for l in wrap(question, inner_width) {
                lines.push(Line::styled(l, Style::new().add_modifier(Modifier::BOLD)));
            }
            lines.push(Line::raw(""));
            for (i, option) in options.iter().enumerate() {
                let style = if i == *selected && input.is_empty() {
                    Style::new().fg(Color::Black).bg(Color::Cyan)
                } else {
                    Style::new()
                };
                lines.push(Line::styled(format!(" {}. {option} ", i + 1), style));
            }
            if !options.is_empty() {
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(vec![
                Span::styled("Answer: ", dim()),
                Span::raw(input.text().replace('\n', " ")),
                Span::styled("▏", Style::new().fg(Color::Cyan)),
            ]));
            let block = Block::bordered()
                .title(" The agent asks ")
                .title_bottom(" ↑↓ select · 1-9 pick · type your own · Enter send · Esc dismiss ")
                .border_style(Style::new().fg(Color::Cyan))
                .padding(ratatui::widgets::Padding::horizontal(1));
            frame.render_widget(Paragraph::new(lines).block(block), rect);
        }
        Modal::Model(picker) => {
            let rows = (rect.height as usize).saturating_sub(8);
            let block = Block::bordered()
                .title(" Choose a model ")
                .title_bottom(" type to filter · ↑↓ choose · Enter switch · Esc cancel ")
                .border_style(Style::new().fg(Color::Cyan))
                .padding(ratatui::widgets::Padding::horizontal(1));
            frame.render_widget(
                Paragraph::new(picker.lines(inner_width, rows)).block(block),
                rect,
            );
        }
        Modal::Help => {
            let keys = [
                ("Enter", "send the message"),
                ("Alt+Enter", "new line"),
                ("/", "commands, with suggestions (Tab completes)"),
                ("Shift+Tab", "cycle modes (plan is read-only)"),
                ("Esc", "cancel the current turn"),
                ("PgUp / PgDn", "scroll the transcript"),
                ("↑ / ↓", "previous messages"),
                ("Ctrl+C", "clear input / cancel / quit (twice)"),
            ];
            let mut help: Vec<(String, &str)> =
                keys.iter().map(|(k, v)| (k.to_string(), *v)).collect();
            help.push((String::new(), ""));
            for c in COMMANDS {
                let usage = if c.args.is_empty() {
                    format!("/{}", c.name)
                } else {
                    format!("/{} {}", c.name, c.args)
                };
                help.push((usage, c.description));
            }
            let lines: Vec<Line> = help
                .iter()
                .map(|(k, v)| {
                    Line::from(vec![
                        Span::styled(
                            format!("{k:<24}"),
                            Style::new().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(*v),
                    ])
                })
                .collect();
            let block = Block::bordered()
                .title(" Help ")
                .title_bottom(" Esc to close ")
                .padding(ratatui::widgets::Padding::horizontal(1));
            frame.render_widget(Paragraph::new(lines).block(block), rect);
        }
    }
}

fn diff_style(line: &str) -> Style {
    if line.starts_with("+++") || line.starts_with("---") {
        dim().add_modifier(Modifier::BOLD)
    } else if line.starts_with('+') {
        Style::new().fg(Color::Green)
    } else if line.starts_with('-') {
        Style::new().fg(Color::Red)
    } else if line.starts_with("@@") {
        Style::new().fg(Color::Cyan)
    } else {
        Style::new()
    }
}

fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

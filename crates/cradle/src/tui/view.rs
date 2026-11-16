//! Rendering.

use cradle_core::{ReviewPolicy, Verdict};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::commands::{COMMANDS, Suggestion};
use super::text::wrap;
use super::theme::Theme;
use super::tools::ToolGroup;
use super::{App, Item, Modal, ToolState};

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_INPUT_LINES: usize = 8;
const MAX_DIFF_LINES: usize = 40;
const MAX_THINKING_LINES: usize = 6;
const SIDEBAR_WIDTH: u16 = 34;
/// Narrower terminals hide the sidebar.
const SIDEBAR_MIN_TERMINAL: u16 = 90;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let theme = &app.theme;
    if let Some(background) = theme.background {
        frame.render_widget(Block::new().style(background), area);
    }
    let show_sidebar = app.sidebar && area.width >= SIDEBAR_MIN_TERMINAL;
    let [header, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let (main, sidebar) = if show_sidebar {
        let [main, sidebar] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(SIDEBAR_WIDTH)]).areas(rest);
        (main, Some(sidebar))
    } else {
        (rest, None)
    };

    let input_width = main.width.saturating_sub(2) as usize;
    let (input_lines, cursor) = app.input.layout(input_width);
    let input_height = input_lines.len().clamp(1, MAX_INPUT_LINES) as u16 + 2;
    let [body, input, footer] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(input_height),
        Constraint::Length(1),
    ])
    .areas(main);

    draw_header(frame, header, app, !show_sidebar);
    draw_transcript(frame, body, app);
    draw_input(frame, input, app, &input_lines, cursor);
    if let Some(sidebar) = sidebar {
        draw_sidebar(frame, sidebar, app);
    }
    frame.render_widget(
        Paragraph::new(
            " Enter send · Shift+Enter newline · / commands · Tab mode · Esc cancel · wheel/PgUp scroll · Ctrl+O tools",
        )
        .style(theme.hint),
        footer,
    );

    if let Some(modal) = &app.modal {
        draw_modal(frame, area, modal, theme);
    } else {
        let suggestions = app.suggestions();
        if !suggestions.is_empty() {
            draw_suggestions(frame, body, input, &suggestions, app.suggest_index, theme);
        }
    }
}

/// Blank out `rect` for a popup, keeping the theme's background if it has one.
fn clear(frame: &mut Frame, rect: Rect, theme: &Theme) {
    frame.render_widget(Clear, rect);
    if let Some(background) = theme.background {
        frame.render_widget(Block::new().style(background), rect);
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
    theme: &Theme,
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
                let on = theme.selection;
                (on.add_modifier(Modifier::BOLD), on)
            } else {
                (theme.title, theme.hint)
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
        .title_bottom(Line::styled(hint, theme.hint))
        .border_style(theme.border);
    clear(frame, rect, theme);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

/// Files changed this session and token usage.
fn draw_sidebar(frame: &mut Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let block = Block::new()
        .borders(ratatui::widgets::Borders::LEFT)
        .border_style(theme.hint)
        .padding(ratatui::widgets::Padding::horizontal(1));
    let width = block.inner(area).width as usize;
    let bold = theme.title;
    let mut lines = vec![Line::styled(
        format!("Changed files ({})", app.files.len()),
        bold,
    )];
    if app.files.is_empty() {
        lines.push(Line::styled("none yet", theme.hint));
    }
    let (mut added, mut removed) = (0, 0);
    for file in &app.files {
        added += file.added;
        removed += file.removed;
        let counts = format!(" +{} -{}", file.added, file.removed);
        let path = truncate_left(&file.path, width.saturating_sub(counts.len()));
        let pad = width.saturating_sub(super::text::display_width(&path) + counts.len());
        lines.push(Line::from(vec![
            Span::raw(format!("{path}{}", " ".repeat(pad))),
            Span::styled(format!(" +{}", file.added), theme.diff_added),
            Span::styled(format!(" -{}", file.removed), theme.diff_removed),
        ]));
    }
    if app.files.len() > 1 {
        lines.push(Line::from(vec![
            Span::styled("total ", theme.hint),
            Span::styled(format!("+{added}"), theme.diff_added),
            Span::styled(format!(" -{removed}"), theme.diff_removed),
        ]));
    }

    let u = &app.usage;
    lines.push(Line::raw(""));
    lines.push(Line::styled("Usage", bold));
    for (label, value) in [
        ("input", u.input_tokens),
        ("output", u.output_tokens),
        ("cache read", u.cache_read_tokens),
        ("cache write", u.cache_write_tokens),
    ] {
        let value = tokens(value);
        let pad = width.saturating_sub(label.len() + value.len());
        lines.push(Line::from(vec![
            Span::styled(format!("{label}{}", " ".repeat(pad)), theme.hint),
            Span::raw(value),
        ]));
    }
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

/// `12345` as `12.3k`.
fn tokens(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// The end of `text` within `width` columns, with `…` in front if cut.
fn truncate_left(text: &str, width: usize) -> String {
    if super::text::display_width(text) <= width {
        return text.to_owned();
    }
    let mut out: Vec<char> = Vec::new();
    let mut used = 1;
    for c in text.chars().rev() {
        let w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        out.push(c);
    }
    std::iter::once('…').chain(out.into_iter().rev()).collect()
}

/// How long ago `secs` (since the Unix epoch) was.
fn ago(secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match now.saturating_sub(secs) {
        d if d < 60 => "just now".to_owned(),
        d if d < 3600 => format!("{}m ago", d / 60),
        d if d < 86_400 => format!("{}h ago", d / 3600),
        d => format!("{}d ago", d / 86_400),
    }
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, show_tokens: bool) {
    let theme = &app.theme;
    let mode_style = if app.read_only {
        theme.mode_read_only
    } else {
        theme.mode_full
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
    let mut spans = vec![
        Span::styled(" cradle ", theme.brand),
        Span::raw(format!(" {}/{} ", app.provider, app.model)),
        Span::styled(mode, mode_style),
        Span::styled(format!("  {review}"), theme.hint),
    ];
    if show_tokens {
        spans.push(Span::styled(
            format!(
                "  tokens in {} out {} cached {}",
                u.input_tokens, u.output_tokens, u.cache_read_tokens
            ),
            theme.hint,
        ));
    }
    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_transcript(frame: &mut Frame, area: Rect, app: &App) {
    let width = (area.width as usize).saturating_sub(2).max(10);
    let mut lines: Vec<Line> = Vec::new();
    for item in &app.items {
        render_item(
            item,
            width,
            app.spinner,
            app.tools_expanded,
            &app.theme,
            &mut lines,
        );
        lines.push(Line::raw(""));
    }
    // While busy, the input box's title shows the spinner and how to cancel.

    let height = area.height as usize;
    let max_scroll = lines.len().saturating_sub(height);
    app.max_scroll.set(max_scroll);
    let scroll = app.scroll.min(max_scroll);
    let start = lines.len().saturating_sub(height + scroll);
    let end = (start + height).min(lines.len());
    let visible: Vec<Line> = lines.drain(start..end).collect();
    frame.render_widget(Paragraph::new(visible), area);
}

/// A tool card. A single call shows its exact action (and, unless it is a read, search or
/// listing that worked, a preview of the result). A group shows counts; expanded, it also
/// lists each file or pattern, with failures in red.
fn render_tools(
    group: &ToolGroup,
    width: usize,
    spinner: usize,
    expanded: bool,
    theme: &Theme,
    out: &mut Vec<Line<'static>>,
) {
    let state = group.state();
    let (mark, mark_style) = match state {
        ToolState::Running => (SPINNER[spinner % SPINNER.len()], theme.tool_running),
        ToolState::Ok => ("✓", theme.tool_ok),
        ToolState::Failed => ("✗", theme.tool_failed),
    };
    let single = group.is_single();
    let mut title = group.title();
    let failed = group.failed_count();
    if failed > 0 && !single {
        title.push_str(&format!(" ({failed} failed)"));
    }
    gutter(
        out,
        &format!("{mark} "),
        mark_style,
        &title,
        Style::new(),
        width,
    );

    let inner = width.saturating_sub(2);
    let red = theme.tool_failed;
    let dim = theme.tool_detail;
    if single {
        let entry = &group.calls[0];
        let failed = entry.state == ToolState::Failed;
        if !entry.preview.is_empty() && (failed || !entry.groups()) {
            let style = if failed { red } else { dim };
            gutter(out, "  │ ", dim, &entry.preview, style, inner);
        }
        return;
    }
    if !expanded {
        return;
    }
    for entry in &group.calls {
        let details = entry
            .activity
            .as_ref()
            .map(|a| a.details.as_slice())
            .unwrap_or_default();
        for detail in details {
            let failure = entry.failed.iter().find(|f| {
                f.strip_prefix(detail.as_str())
                    .is_some_and(|rest| rest.starts_with(": "))
            });
            match failure {
                Some(f) => gutter(out, "  │ ✗ ", red, f, red, inner),
                None if entry.state == ToolState::Failed => {
                    gutter(out, "  │ ✗ ", red, detail, red, inner)
                }
                None => gutter(out, "  │ ", dim, detail, dim, inner),
            }
        }
        if entry.state == ToolState::Failed && !entry.preview.is_empty() {
            gutter(out, "  │   ", dim, &entry.preview, red, inner);
        }
    }
}

fn render_item(
    item: &Item,
    width: usize,
    spinner: usize,
    expanded: bool,
    theme: &Theme,
    out: &mut Vec<Line<'static>>,
) {
    match item {
        Item::User(text) => gutter(out, "› ", theme.user_mark, text, theme.user, width),
        Item::Assistant(text) => render_markdown(text, width, theme, out),
        Item::Thinking(text) => {
            let wrapped = wrap(text.trim(), width);
            let skip = wrapped.len().saturating_sub(MAX_THINKING_LINES);
            let style = theme.thinking;
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
        Item::Tools(group) => render_tools(group, width, spinner, expanded, theme, out),
        Item::Diff { title, diff } => {
            out.push(Line::styled(format!("  {title}"), theme.diff_header));
            let lines: Vec<&str> = diff.lines().collect();
            for line in lines.iter().take(MAX_DIFF_LINES) {
                for part in wrap(line, width.saturating_sub(2)) {
                    out.push(Line::styled(format!("  {part}"), diff_style(line, theme)));
                }
            }
            if lines.len() > MAX_DIFF_LINES {
                out.push(Line::styled(
                    format!(
                        "  … {} more lines (/diff shows all changes)",
                        lines.len() - MAX_DIFF_LINES
                    ),
                    theme.hint,
                ));
            }
        }
        Item::Review {
            command,
            verdict,
            reason,
        } => {
            let (word, style) = match verdict {
                Verdict::Safe => ("safe, running", theme.review_safe),
                Verdict::Unsafe => ("unsafe, asking you", theme.review_unsafe),
                Verdict::Unsure => ("unsure, asking you", theme.review_unsure),
            };
            gutter(
                out,
                "◆ ",
                style,
                &format!("reviewer: `{command}` is {word}. {reason}"),
                style,
                width,
            );
        }
        Item::Notice(text) => gutter(out, "· ", theme.notice, text, theme.notice, width),
        Item::Error(text) => gutter(out, "! ", theme.error_mark, text, theme.error, width),
    }
}

/// Model output, rendered as markdown with a two-column margin.
fn render_markdown(text: &str, width: usize, theme: &Theme, out: &mut Vec<Line<'static>>) {
    for line in super::markdown::render(text, width, theme) {
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
        app.theme.input_read_only
    } else {
        app.theme.input_full
    };
    let block = Block::bordered().title(title).border_style(border);
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

fn draw_modal(frame: &mut Frame, area: Rect, modal: &Modal, theme: &Theme) {
    // Never larger than the terminal, however small it is.
    let width = area.width.saturating_sub(4).min(110);
    let height = area.height.saturating_sub(4);
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    clear(frame, rect, theme);
    let inner_width = width.saturating_sub(4) as usize;

    match modal {
        Modal::Approval {
            title,
            body,
            note,
            always,
            scroll,
            feedback,
            ..
        } => {
            let mut lines: Vec<Line> = Vec::new();
            if let Some(note) = note {
                for l in wrap(note, inner_width) {
                    lines.push(Line::styled(l, theme.note));
                }
                lines.push(Line::raw(""));
            }
            for raw in body.lines() {
                for l in wrap(raw, inner_width) {
                    lines.push(Line::styled(l, diff_style(raw, theme)));
                }
            }
            let keys = match (feedback, always) {
                (Some(_), _) => " Enter send · Shift+Enter new line · Esc back ".to_owned(),
                (None, Some(always)) => format!(
                    " [y] allow once · [a] {always} · [c] change something · [n] no · ↑↓ scroll "
                ),
                (None, None) => {
                    " [y] allow once · [c] change something · [n] no · ↑↓ scroll ".to_owned()
                }
            };
            let block = Block::bordered()
                .title(format!(" {title} "))
                .title_bottom(Line::styled(keys, theme.title))
                .border_style(theme.border_approval)
                .padding(ratatui::widgets::Padding::horizontal(1));
            let inner = block.inner(rect);
            frame.render_widget(block, rect);

            // While typing feedback, it takes the bottom of the dialog.
            let feedback_lines: Vec<Line> = match feedback {
                Some(input) => {
                    let mut out = vec![Line::styled(
                        "What should the agent do instead?",
                        theme.prompt,
                    )];
                    let (text, _) = input.layout(inner.width.saturating_sub(2) as usize);
                    let last = text.len() - 1;
                    for (i, l) in text.into_iter().enumerate() {
                        let mut spans = vec![Span::styled("› ", theme.cursor), Span::raw(l)];
                        if i == last {
                            spans.push(Span::styled("▏", theme.cursor));
                        }
                        out.push(Line::from(spans));
                    }
                    out
                }
                None => Vec::new(),
            };
            let reserved = (feedback_lines.len() as u16 + 1).min(inner.height)
                * u16::from(!feedback_lines.is_empty());
            let body_area = Rect {
                height: inner.height - reserved,
                ..inner
            };
            let visible = body_area.height as usize;
            let scroll = (*scroll).min(lines.len().saturating_sub(visible));
            let lines: Vec<Line> = lines.into_iter().skip(scroll).collect();
            frame.render_widget(Paragraph::new(lines), body_area);
            if reserved > 0 {
                let area = Rect {
                    y: inner.y + inner.height - reserved + 1,
                    height: reserved - 1,
                    ..inner
                };
                frame.render_widget(Paragraph::new(feedback_lines), area);
            }
        }
        Modal::Question(questions) => {
            let rows = rect.height.saturating_sub(2) as usize;
            let block = Block::bordered()
                .title(questions.title())
                .title_bottom(questions.hint())
                .border_style(theme.border)
                .padding(ratatui::widgets::Padding::horizontal(1));
            frame.render_widget(
                Paragraph::new(questions.lines(inner_width, rows, theme)).block(block),
                rect,
            );
        }
        Modal::Model(picker) => {
            let rows = (rect.height as usize).saturating_sub(8);
            let block = Block::bordered()
                .title(" Choose a model ")
                .title_bottom(" type to filter · ↑↓ choose · Enter switch · Esc cancel ")
                .border_style(theme.border)
                .padding(ratatui::widgets::Padding::horizontal(1));
            frame.render_widget(
                Paragraph::new(picker.lines(inner_width, rows, theme)).block(block),
                rect,
            );
        }
        Modal::Session { sessions, selected } => {
            let rows = (rect.height as usize).saturating_sub(2).max(1);
            let first = (selected + 1).saturating_sub(rows);
            let lines: Vec<Line> = sessions
                .iter()
                .enumerate()
                .skip(first)
                .take(rows)
                .map(|(i, s)| {
                    let meta = format!("{:>9} · {:>3} msgs  ", ago(s.started), s.messages);
                    let title = super::text::wrap(&s.title, inner_width.saturating_sub(meta.len()))
                        .into_iter()
                        .next()
                        .unwrap_or_default();
                    let style = if i == *selected {
                        theme.selection
                    } else {
                        Style::new()
                    };
                    Line::from(vec![
                        Span::styled(meta, if i == *selected { style } else { theme.hint }),
                        Span::styled(title, style),
                    ])
                })
                .collect();
            // The id is for `cradle --resume <id>`.
            let id = sessions
                .get(*selected)
                .map(|s| format!("id {} · ", s.id))
                .unwrap_or_default();
            let block = Block::bordered()
                .title(" Resume a conversation ")
                .title_bottom(format!(" {id}↑↓ choose · Enter resume · Esc cancel "))
                .border_style(theme.border)
                .padding(ratatui::widgets::Padding::horizontal(1));
            frame.render_widget(Paragraph::new(lines).block(block), rect);
        }
        Modal::Help => {
            let keys = [
                ("Enter", "send the message"),
                ("Shift+Enter", "new line (also Alt+Enter, Ctrl+J)"),
                ("/", "commands, with suggestions (Tab completes)"),
                (
                    "Tab / Shift+Tab",
                    "cycle modes forward / back (plan is read-only)",
                ),
                ("Esc", "cancel the current turn"),
                ("Wheel, PgUp / PgDn", "scroll the transcript"),
                ("Ctrl+O", "expand / collapse grouped tool calls"),
                ("↑ / ↓", "previous messages"),
                ("Cmd+⌫ / Ctrl+U", "delete to the start of the line"),
                ("Alt+⌫ / Ctrl+W", "delete the previous word"),
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
                        Span::styled(format!("{k:<24}"), theme.title),
                        Span::raw(*v),
                    ])
                })
                .collect();
            let block = Block::bordered()
                .title(" Help ")
                .title_bottom(" Esc to close ")
                .border_style(theme.border_help)
                .padding(ratatui::widgets::Padding::horizontal(1));
            frame.render_widget(Paragraph::new(lines).block(block), rect);
        }
    }
}

fn diff_style(line: &str, theme: &Theme) -> Style {
    if line.starts_with("+++") || line.starts_with("---") {
        theme.diff_header
    } else if line.starts_with('+') {
        theme.diff_added
    } else if line.starts_with('-') {
        theme.diff_removed
    } else if line.starts_with("@@") {
        theme.diff_hunk
    } else {
        Style::new()
    }
}

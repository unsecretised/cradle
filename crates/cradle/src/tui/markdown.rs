//! Renders the model's markdown as styled terminal lines.
//!
//! Supports headings, emphasis, strikethrough, inline code, links, fenced and indented
//! code blocks, nested bullet/numbered/task lists, block quotes, rules and tables.
//! Wrapping keeps each word's style. Incomplete markdown (mid-stream) renders as far as
//! it parses.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::text::display_width;
use super::theme::Theme;

pub fn render(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut r = Renderer::new(width.max(10), theme);
    for event in Parser::new_ext(text, options) {
        r.event(event);
    }
    r.finish()
}

struct List {
    /// Next number for an ordered list, `None` for bullets.
    next: Option<u64>,
}

#[derive(Default)]
struct Table {
    rows: Vec<Vec<String>>,
    header_rows: usize,
    cell: Option<String>,
    in_head: bool,
}

struct Renderer<'t> {
    theme: &'t Theme,
    width: usize,
    out: Vec<Line<'static>>,
    /// Inline content of the block being built.
    spans: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<List>,
    /// Marker for the first line of the current list item ("• ", "1. ").
    item_marker: Option<String>,
    quote_depth: usize,
    /// `Some(lang)` inside a code block.
    code: Option<String>,
    links: Vec<String>,
    table: Option<Table>,
}

impl<'t> Renderer<'t> {
    fn new(width: usize, theme: &'t Theme) -> Self {
        Renderer {
            theme,
            width,
            out: Vec::new(),
            spans: Vec::new(),
            styles: vec![Style::new()],
            lists: Vec::new(),
            item_marker: None,
            quote_depth: 0,
            code: None,
            links: Vec::new(),
            table: None,
        }
    }

    fn style(&self) -> Style {
        *self.styles.last().expect("base style")
    }

    /// Code frames, quote bars, rules, table borders and link URLs.
    fn dim(&self) -> Style {
        self.theme.md_punctuation
    }

    fn push_style(&mut self, f: impl FnOnce(Style) -> Style) {
        self.styles.push(f(self.style()));
    }

    fn event(&mut self, event: Event<'_>) {
        if let Some(table) = &mut self.table {
            match &event {
                Event::Text(t) | Event::Code(t) => {
                    if let Some(cell) = &mut table.cell {
                        cell.push_str(t);
                    }
                    return;
                }
                Event::Start(Tag::TableHead) => {
                    table.in_head = true;
                    table.rows.push(Vec::new());
                    return;
                }
                Event::End(TagEnd::TableHead) => {
                    table.in_head = false;
                    table.header_rows = table.rows.len();
                    return;
                }
                Event::Start(Tag::TableRow) => {
                    table.rows.push(Vec::new());
                    return;
                }
                Event::Start(Tag::TableCell) => {
                    table.cell = Some(String::new());
                    return;
                }
                Event::End(TagEnd::TableCell) => {
                    let cell = table.cell.take().unwrap_or_default();
                    if let Some(row) = table.rows.last_mut() {
                        row.push(cell);
                    }
                    return;
                }
                Event::End(TagEnd::Table) => {
                    let table = self.table.take().unwrap_or_default();
                    self.render_table(table);
                    self.blank();
                    return;
                }
                _ => return, // inline styling inside cells is flattened
            }
        }

        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.code.is_some() {
                    self.code_text(&text);
                } else {
                    self.spans
                        .push(Span::styled(text.into_string(), self.style()));
                }
            }
            Event::Code(code) => {
                let style = self.style().patch(self.theme.md_code);
                self.spans.push(Span::styled(code.into_string(), style));
            }
            Event::SoftBreak => self.spans.push(Span::styled(" ", self.style())),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.out
                    .push(Line::styled("─".repeat(self.width.min(60)), self.dim()));
                self.blank();
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                self.spans.push(Span::styled(mark, self.dim()));
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                self.spans
                    .push(Span::styled(html.into_string(), self.dim()));
            }
            Event::FootnoteReference(name) => {
                self.spans
                    .push(Span::styled(format!("[^{name}]"), self.dim()));
            }
            Event::InlineMath(m) | Event::DisplayMath(m) => {
                self.spans
                    .push(Span::styled(m.into_string(), self.theme.md_code));
            }
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.flush();
                let style = match level {
                    HeadingLevel::H1 => self.theme.md_h1,
                    HeadingLevel::H2 => self.theme.md_h2,
                    _ => self.theme.md_heading,
                };
                self.styles.push(style);
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.quote_depth += 1;
                let quote = self.theme.md_quote;
                self.push_style(|s| s.patch(quote));
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        lang.split_whitespace().next().unwrap_or("").to_owned()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                let label = if lang.is_empty() {
                    "code".to_owned()
                } else {
                    lang.clone()
                };
                let prefix = self.prefix(false);
                let mut spans = prefix;
                spans.push(Span::styled(format!("┌ {label}"), self.dim()));
                self.out.push(Line::from(spans));
                self.code = Some(lang);
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(List { next: start });
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(List { next: Some(n) }) => {
                        let marker = format!("{n}. ");
                        *n += 1;
                        marker
                    }
                    _ => "• ".to_owned(),
                };
                self.item_marker = Some(marker);
            }
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.links.push(dest_url.into_string());
                let link = self.theme.md_link;
                self.push_style(|s| s.patch(link));
            }
            Tag::Image { dest_url, .. } => {
                self.links.push(dest_url.into_string());
                self.spans.push(Span::styled("[image: ", self.dim()));
                let image = self.theme.md_image;
                self.push_style(|s| s.patch(image));
            }
            Tag::Table(_) => {
                self.flush();
                self.table = Some(Table::default());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                // Items in a tight list sit on consecutive lines.
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Heading(_) => {
                self.flush();
                self.styles.pop();
                self.blank();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.styles.pop();
                self.quote_depth = self.quote_depth.saturating_sub(1);
                if self.quote_depth == 0 && self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::CodeBlock => {
                let mut spans = self.prefix(false);
                spans.push(Span::styled("└", self.dim()));
                self.out.push(Line::from(spans));
                self.code = None;
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => self.flush(),
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.styles.pop();
            }
            TagEnd::Link => {
                self.styles.pop();
                if let Some(url) = self.links.pop() {
                    let text: String = self.spans.iter().map(|s| s.content.as_ref()).collect();
                    if !url.is_empty() && !text.ends_with(&url) {
                        self.spans
                            .push(Span::styled(format!(" ({url})"), self.dim()));
                    }
                }
            }
            TagEnd::Image => {
                self.styles.pop();
                let url = self.links.pop().unwrap_or_default();
                self.spans
                    .push(Span::styled(format!(" {url}]"), self.dim()));
            }
            _ => {}
        }
    }

    /// Quote bars and list indentation for the next line. `first` uses the item marker.
    fn prefix(&mut self, first: bool) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        for _ in 0..self.quote_depth {
            spans.push(Span::styled("│ ", self.dim()));
        }
        if !self.lists.is_empty() {
            let depth = self.lists.len() - 1;
            if depth > 0 {
                spans.push(Span::raw("  ".repeat(depth)));
            }
            match (first, &self.item_marker) {
                (true, Some(marker)) => {
                    spans.push(Span::styled(marker.clone(), self.theme.md_list_marker));
                    self.item_marker = None;
                }
                _ => spans.push(Span::raw("  ")),
            }
        }
        spans
    }

    /// Wrap and emit the pending inline content.
    fn flush(&mut self) {
        if self.spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.spans);
        let first = self.prefix(true);
        let rest = self.prefix(false);
        self.out.extend(wrap_spans(&spans, self.width, first, rest));
    }

    fn code_text(&mut self, text: &str) {
        for line in text.trim_end_matches('\n').split('\n') {
            let mut prefix = self.prefix(false);
            prefix.push(Span::styled("│ ", self.dim()));
            let room = self.width.saturating_sub(spans_width(&prefix)).max(4);
            // Code is hard-wrapped, never reflowed.
            for chunk in hard_wrap(&line.replace('\t', "    "), room) {
                let mut spans = prefix.clone();
                spans.push(Span::styled(chunk, self.theme.md_code_block));
                self.out.push(Line::from(spans));
            }
        }
    }

    fn render_table(&mut self, table: Table) {
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        if columns == 0 {
            return;
        }
        let mut widths = vec![0; columns];
        for row in &table.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(display_width(cell));
            }
        }
        let total: usize = widths.iter().sum::<usize>() + 3 * columns.saturating_sub(1);
        let prefix = self.prefix(false);
        if total + spans_width(&prefix) > self.width {
            // Too wide for the terminal: one "a · b · c" paragraph per row.
            for (i, row) in table.rows.iter().enumerate() {
                let style = if i < table.header_rows {
                    self.theme.md_table_header
                } else {
                    Style::new()
                };
                let spans = vec![Span::styled(row.join(" · "), style)];
                let first = self.prefix(false);
                let rest = self.prefix(false);
                self.out.extend(wrap_spans(&spans, self.width, first, rest));
            }
            return;
        }
        for (i, row) in table.rows.iter().enumerate() {
            let mut spans = prefix.clone();
            for (c, width) in widths.iter().enumerate() {
                let cell = row.get(c).map(String::as_str).unwrap_or("");
                let pad = width - display_width(cell);
                let style = if i < table.header_rows {
                    self.theme.md_table_header
                } else {
                    Style::new()
                };
                spans.push(Span::styled(format!("{cell}{}", " ".repeat(pad)), style));
                if c + 1 < columns {
                    spans.push(Span::styled(" │ ", self.dim()));
                }
            }
            self.out.push(Line::from(spans));
            if i + 1 == table.header_rows {
                let mut rule = prefix.clone();
                let line = widths
                    .iter()
                    .map(|w| "─".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                rule.push(Span::styled(line, self.dim()));
                self.out.push(Line::from(rule));
            }
        }
    }

    fn blank(&mut self) {
        if self
            .out
            .last()
            .is_some_and(|l| !l.spans.is_empty() || l.width() > 0)
        {
            self.out.push(Line::raw(""));
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        self.flush();
        if let Some(table) = self.table.take() {
            self.render_table(table);
        }
        while self.out.last().is_some_and(|l| l.width() == 0) {
            self.out.pop();
        }
        self.out
    }
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| display_width(&s.content)).sum()
}

/// Split into chunks of at most `width` columns.
fn hard_wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut col = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if col + w > width && col > 0 {
            out.push(String::new());
            col = 0;
        }
        out.last_mut().unwrap().push(c);
        col += w;
    }
    out
}

/// Word-wrap styled spans. `first` prefixes the first line, `rest` the others.
fn wrap_spans(
    spans: &[Span<'static>],
    width: usize,
    first: Vec<Span<'static>>,
    rest: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    // Words and runs of spaces, each with its style.
    let mut tokens: Vec<(String, Style, bool)> = Vec::new();
    for span in spans {
        let mut current = String::new();
        let mut current_space = false;
        for c in span.content.chars() {
            let space = c == ' ' || c == '\n';
            if !current.is_empty() && space != current_space {
                tokens.push((std::mem::take(&mut current), span.style, current_space));
            }
            current_space = space;
            current.push(if c == '\n' { ' ' } else { c });
        }
        if !current.is_empty() {
            tokens.push((current, span.style, current_space));
        }
    }

    let mut lines = Vec::new();
    let mut line = first;
    let rest_width = spans_width(&rest);
    let mut used = spans_width(&line);
    let mut content = false;
    let mut pending_space: Option<Style> = None;

    for (text, style, is_space) in tokens {
        if is_space {
            if content {
                pending_space = Some(style);
            }
            continue;
        }
        let w = display_width(&text);
        let space = usize::from(pending_space.is_some());
        if content && used + space + w > width {
            lines.push(Line::from(std::mem::replace(&mut line, rest.clone())));
            used = rest_width;
            pending_space = None;
        }
        if let Some(space_style) = pending_space.take() {
            line.push(Span::styled(" ", space_style));
            used += 1;
        }
        if used + w > width {
            // A single word longer than the line: break it.
            let room = width.saturating_sub(used).max(1);
            let mut chunks = hard_wrap(&text, room).into_iter();
            if let Some(head) = chunks.next() {
                used += display_width(&head);
                line.push(Span::styled(head, style));
            }
            for chunk in chunks {
                lines.push(Line::from(std::mem::replace(&mut line, rest.clone())));
                used = rest_width + display_width(&chunk);
                line.push(Span::styled(chunk, style));
            }
        } else {
            line.push(Span::styled(text, style));
            used += w;
        }
        content = true;
    }
    if content {
        lines.push(Line::from(line));
    }
    lines
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    /// Rendered with the default (dark) theme.
    fn render(text: &str, width: usize) -> Vec<Line<'static>> {
        super::render(text, width, &Theme::default())
    }

    fn text(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    fn find<'a>(lines: &'a [Line<'static>], content: &str) -> &'a Span<'static> {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .find(|s| s.content == content)
            .unwrap_or_else(|| panic!("no span {content:?}"))
    }

    #[test]
    fn inline_styles() {
        let lines = render("Some **bold**, *italic*, `code` and ~~gone~~.", 80);
        assert_eq!(text(&lines), ["Some bold, italic, code and gone."]);
        assert!(
            find(&lines, "bold")
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
        assert!(
            find(&lines, "italic")
                .style
                .add_modifier
                .contains(Modifier::ITALIC)
        );
        assert_eq!(find(&lines, "code").style.fg, Some(Color::Yellow));
        assert!(
            find(&lines, "gone")
                .style
                .add_modifier
                .contains(Modifier::CROSSED_OUT)
        );
    }

    #[test]
    fn headings_paragraphs_and_rules() {
        let lines = render("# Title\n\nFirst para.\n\n---\n\nSecond.", 80);
        let t = text(&lines);
        assert_eq!(t[0], "Title");
        assert_eq!(t[1], "");
        assert_eq!(t[2], "First para.");
        assert!(t.iter().any(|l| l.starts_with("───")));
        assert_eq!(t.last().unwrap(), "Second.");
        assert!(
            find(&lines, "Title")
                .style
                .add_modifier
                .contains(Modifier::UNDERLINED)
        );
    }

    #[test]
    fn nested_lists_and_tasks() {
        let lines = render("- one\n- two\n  1. a\n  2. b\n- [x] done", 80);
        assert_eq!(
            text(&lines),
            ["• one", "• two", "  1. a", "  2. b", "• [x] done"]
        );
    }

    #[test]
    fn code_blocks_keep_layout() {
        let lines = render("```rust\nfn main() {\n    run();\n}\n```", 80);
        assert_eq!(
            text(&lines),
            ["┌ rust", "│ fn main() {", "│     run();", "│ }", "└"]
        );
        assert_eq!(find(&lines, "    run();").style.fg, Some(Color::Yellow));
    }

    #[test]
    fn links_show_their_url() {
        let lines = render("See [the docs](https://example.com).", 80);
        assert_eq!(text(&lines), ["See the docs (https://example.com)."]);
        let lines = render("<https://example.com>", 80);
        assert_eq!(
            text(&lines),
            ["https://example.com"],
            "autolinks are not repeated"
        );
    }

    #[test]
    fn wrapping_keeps_styles_and_indent() {
        let lines = render("- alpha **beta gamma** delta", 12);
        assert_eq!(text(&lines), ["• alpha beta", "  gamma", "  delta"]);
        let gamma = find(&lines, "gamma");
        assert!(gamma.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn quotes_and_tables() {
        let lines = render("> quoted\n\n| a | bb |\n|---|---|\n| 1 | 2 |", 80);
        let t = text(&lines);
        assert_eq!(t[0], "│ quoted");
        assert!(t.contains(&"a │ bb".to_owned()));
        assert!(t.contains(&"1 │ 2 ".to_owned()));
    }

    #[test]
    fn unfinished_markdown_still_renders() {
        let lines = render("Working on it:\n\n```py\nprint(1", 80);
        // The parser closes an open fence at the end of the text.
        assert_eq!(
            text(&lines),
            ["Working on it:", "", "┌ py", "│ print(1", "└"]
        );
    }
}

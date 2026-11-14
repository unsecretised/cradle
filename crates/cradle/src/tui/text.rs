//! Text layout helpers: word wrapping by display width, and the input line editor.

use unicode_width::UnicodeWidthChar;

/// Word-wrap `text` to `width` columns. Newlines are kept; tabs become four spaces.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for line in text.replace('\t', "    ").split('\n') {
        let mut current = String::new();
        let mut current_width = 0;
        // Byte index just after the last space in `current`, if any.
        let mut break_at: Option<usize> = None;
        for c in line.chars() {
            let w = c.width().unwrap_or(0);
            if current_width + w > width && !current.is_empty() {
                if c == ' ' {
                    // The line is full exactly at a word boundary: break here.
                    out.push(std::mem::take(&mut current).trim_end().to_owned());
                    current_width = 0;
                    break_at = None;
                    continue;
                }
                match break_at {
                    Some(at) if at < current.len() => {
                        let rest = current.split_off(at);
                        out.push(current.trim_end().to_owned());
                        current_width = display_width(&rest);
                        current = rest;
                    }
                    _ => {
                        out.push(std::mem::take(&mut current));
                        current_width = 0;
                    }
                }
                break_at = None;
            }
            current.push(c);
            current_width += w;
            if c == ' ' {
                break_at = Some(current.len());
            }
        }
        out.push(current);
    }
    out
}

pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// A multi-line text input with a cursor (a byte index into `text`).
#[derive(Debug, Default)]
pub struct Input {
    text: String,
    cursor: usize,
}

impl Input {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }

    pub fn take(&mut self) -> String {
        self.cursor = 0;
        std::mem::take(&mut self.text)
    }

    pub fn insert(&mut self, c: char) {
        self.text.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn backspace(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    pub fn delete(&mut self) {
        if self.cursor < self.text.len() {
            self.text.remove(self.cursor);
        }
    }

    /// Delete from the start of the current line to the cursor (or the newline before it,
    /// at the start of a line).
    pub fn delete_to_line_start(&mut self) {
        let start = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        if start == self.cursor {
            self.backspace();
            return;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    /// Delete the word before the cursor, and any spaces after it.
    pub fn delete_word(&mut self) {
        let before = &self.text[..self.cursor];
        let trimmed = before.trim_end_matches([' ', '\t']);
        let start = trimmed
            .rfind(|c: char| c.is_whitespace())
            .map_or(0, |i| i + 1);
        let start = if start == self.cursor && start > 0 {
            // Only a newline before the cursor: delete it.
            start - 1
        } else {
            start
        };
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    pub fn left(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
        }
    }

    pub fn right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
    }

    pub fn end(&mut self) {
        self.cursor += self.text[self.cursor..]
            .find('\n')
            .unwrap_or(self.text.len() - self.cursor);
    }

    /// Lay out the text in `width` columns (hard wrap). Returns the lines and the cursor's
    /// (row, column).
    pub fn layout(&self, width: usize) -> (Vec<String>, (usize, usize)) {
        let width = width.max(1);
        let mut lines = vec![String::new()];
        let mut col = 0;
        let mut cursor = (0, 0);
        for (i, c) in self.text.char_indices() {
            if i == self.cursor {
                cursor = (lines.len() - 1, col);
            }
            if c == '\n' {
                lines.push(String::new());
                col = 0;
                continue;
            }
            let w = c.width().unwrap_or(0);
            if col + w > width {
                lines.push(String::new());
                col = 0;
            }
            lines.last_mut().unwrap().push(c);
            col += w;
        }
        if self.cursor >= self.text.len() {
            if col >= width {
                lines.push(String::new());
                col = 0;
            }
            cursor = (lines.len() - 1, col);
        }
        (lines, cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_word_boundaries() {
        assert_eq!(wrap("hello world foo", 11), ["hello world", "foo"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\n\nb", 10), ["a", "", "b"]);
        assert_eq!(wrap("", 10), [""]);
    }

    #[test]
    fn input_editing_and_layout() {
        let mut input = Input::default();
        for c in "héllo".chars() {
            input.insert(c);
        }
        input.left();
        input.backspace();
        assert_eq!(input.text(), "hélo");
        input.end();
        input.insert('\n');
        input.insert('x');
        let (lines, cursor) = input.layout(10);
        assert_eq!(lines, ["hélo", "x"]);
        assert_eq!(cursor, (1, 1));
        input.home();
        assert_eq!(input.layout(10).1, (1, 0));
        assert_eq!(input.take(), "hélo\nx");
        assert!(input.is_empty());
    }

    #[test]
    fn deletes_lines_and_words() {
        let mut input = Input::default();
        input.insert_str("one two  \nthree four");
        input.delete_word();
        assert_eq!(input.text(), "one two  \nthree ");
        input.delete_to_line_start();
        assert_eq!(input.text(), "one two  \n");
        input.delete_to_line_start();
        assert_eq!(
            input.text(),
            "one two  ",
            "at a line start, joins the lines"
        );
        input.delete_word();
        assert_eq!(input.text(), "one ");
        input.delete_to_line_start();
        assert!(input.is_empty());
    }
}

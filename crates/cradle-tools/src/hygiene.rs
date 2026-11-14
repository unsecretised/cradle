//! Makes hidden text visible before it reaches the model or the user.
//!
//! Invisible characters (zero-width, bidi controls, Unicode tags, ...) can carry
//! instructions a reviewer never sees. Tool output is rewritten so each one shows as
//! `<U+XXXX>`. See `.agents/roadmap/FEATURE_CONTENT_HYGIENE.md`.

use std::borrow::Cow;
use std::fmt::Write;

/// Whether `c` is invisible or reorders text, and so must be shown explicitly.
pub fn is_suspicious(c: char) -> bool {
    matches!(c,
        '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}'
        | '\u{7F}'..='\u{9F}'
        | '\u{AD}' | '\u{34F}' | '\u{61C}' | '\u{115F}' | '\u{1160}'
        | '\u{17B4}' | '\u{17B5}' | '\u{180B}'..='\u{180F}'
        | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}'
        | '\u{3164}' | '\u{FE00}'..='\u{FE0F}' | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF0}'..='\u{FFFB}'
        | '\u{E000}'..='\u{F8FF}'
        | '\u{E0000}'..='\u{E0FFF}'
        | '\u{F0000}'..='\u{10FFFF}')
}

/// Replace every suspicious character with `<U+XXXX>`. `\n`, `\r` and `\t` are kept.
pub fn reveal_invisible(s: &str) -> Cow<'_, str> {
    if !s.chars().any(is_suspicious) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        if is_suspicious(c) {
            write!(out, "<U+{:04X}>", c as u32).unwrap();
        } else {
            out.push(c);
        }
    }
    Cow::Owned(out)
}

/// Remove ANSI escape sequences (colours, cursor movement) from command output.
pub fn strip_ansi(s: &str) -> Cow<'_, str> {
    if !s.contains('\u{1b}') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: ESC [ params final-byte
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL or ESC \
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' || (c == '\u{1b}' && chars.peek() == Some(&'\\')) {
                        if c == '\u{1b}' {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    Cow::Owned(out)
}

/// Prepare untrusted text for the model: strip terminal escapes, reveal hidden characters.
pub fn clean(s: &str) -> String {
    reveal_invisible(&strip_ansi(s)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reveals_hidden_characters() {
        assert_eq!(reveal_invisible("plain\ttext\n"), "plain\ttext\n");
        assert_eq!(reveal_invisible("a\u{200B}b"), "a<U+200B>b");
        assert_eq!(reveal_invisible("x\u{202E}y"), "x<U+202E>y");
        // "hi" spelled in Unicode tag characters.
        assert_eq!(reveal_invisible("\u{E0068}\u{E0069}"), "<U+E0068><U+E0069>");
        assert_eq!(
            reveal_invisible("é — ✓"),
            "é — ✓",
            "visible non-ASCII is untouched"
        );
    }

    #[test]
    fn strips_ansi() {
        assert_eq!(strip_ansi("\u{1b}[1;31merror\u{1b}[0m: x"), "error: x");
        assert_eq!(
            strip_ansi("\u{1b}]8;;http://x\u{7}link\u{1b}]8;;\u{7}"),
            "link"
        );
    }
}

//! A minimal single-line editor (insert and command modes).

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineEditor {
    text: String,
    /// Cursor position as a byte index (always on a char boundary).
    cursor: usize,
}

pub enum EditResult {
    Continue,
    Submit,
    Cancel,
}

impl LineEditor {
    pub fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
            cursor: text.len(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Cursor position as a byte index.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replace the text, the cursor at `cursor` (clamped to the text).
    pub fn set(&mut self, text: String, cursor: usize) {
        self.cursor = cursor.min(text.len());
        while !text.is_char_boundary(self.cursor) {
            self.cursor -= 1;
        }
        self.text = text;
    }

    /// Display width of the text before the cursor.
    pub fn cursor_col(&self) -> usize {
        self.text[..self.cursor].width()
    }

    fn prev_boundary(&self) -> usize {
        self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_boundary(&self) -> usize {
        self.text[self.cursor..]
            .chars()
            .next()
            .map(|c| self.cursor + c.len_utf8())
            .unwrap_or(self.cursor)
    }

    pub fn handle(&mut self, key: KeyEvent) -> EditResult {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => return EditResult::Submit,
            KeyCode::Esc => return EditResult::Cancel,
            KeyCode::Char('c') if ctrl => return EditResult::Cancel,
            KeyCode::Char('a') if ctrl => self.cursor = 0,
            KeyCode::Char('e') if ctrl => self.cursor = self.text.len(),
            KeyCode::Char('u') if ctrl => {
                self.text.drain(..self.cursor);
                self.cursor = 0;
            }
            KeyCode::Char('k') if ctrl => self.text.truncate(self.cursor),
            KeyCode::Char('w') if ctrl => {
                let before = &self.text[..self.cursor];
                let trimmed = before.trim_end();
                let start = trimmed.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
                self.text.drain(start..self.cursor);
                self.cursor = start;
            }
            KeyCode::Char(c) if !ctrl => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let p = self.prev_boundary();
                    self.text.drain(p..self.cursor);
                    self.cursor = p;
                }
            }
            KeyCode::Delete => {
                let n = self.next_boundary();
                self.text.drain(self.cursor..n);
            }
            KeyCode::Left => self.cursor = self.prev_boundary(),
            KeyCode::Right => self.cursor = self.next_boundary(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.text.len(),
            _ => {}
        }
        EditResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn editing() {
        let mut e = LineEditor::new("héllo");
        e.handle(k(KeyCode::Backspace));
        e.handle(k(KeyCode::Left));
        e.handle(k(KeyCode::Left));
        e.handle(k(KeyCode::Char('X')));
        assert_eq!(e.text(), "héXll");
        assert_eq!(e.cursor_col(), 3);
        e.handle(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(e.text(), "ll");
    }
}

//! Small multi-line text editor used for notes in the terminal apps (no external editor):
//! Enter = new line, Ctrl+Enter (or Ctrl+S / Ctrl+D) = save, Esc = cancel.
//! `TextArea` is pure (no terminal access); `run_inline` is the CLI front end.

use ratatui::crossterm::{
    cursor::{MoveDown, MoveToColumn, MoveUp},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};
use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct TextArea {
    pub lines: Vec<String>,
    pub row: usize,
    /// Cursor column as a character index in `lines[row]`.
    pub col: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Save,
    Cancel,
}

impl TextArea {
    pub fn new(text: &str) -> TextArea {
        let mut lines: Vec<String> = text.replace("\r\n", "\n").split('\n').map(String::from).collect();
        if lines.len() > 1 && lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop(); // trailing newline
        }
        let row = lines.len() - 1;
        let col = lines[row].chars().count();
        TextArea { lines, row, col }
    }

    /// The text, lines joined by `\n`, with a trailing newline unless empty.
    pub fn text(&self) -> String {
        if self.lines.iter().all(|l| l.is_empty()) {
            return String::new();
        }
        let mut s = self.lines.join("\n");
        s.push('\n');
        s
    }

    fn len(&self) -> usize {
        self.lines[self.row].chars().count()
    }

    fn byte(&self, col: usize) -> usize {
        self.lines[self.row].char_indices().nth(col).map_or(self.lines[self.row].len(), |(b, _)| b)
    }

    pub fn handle_key(&mut self, k: KeyEvent) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Esc => return Outcome::Cancel,
            KeyCode::Enter if ctrl => return Outcome::Save,
            KeyCode::Char('s') | KeyCode::Char('d') if ctrl && !alt => return Outcome::Save,
            KeyCode::Char('c') | KeyCode::Char('q') if ctrl && !alt => return Outcome::Cancel,
            KeyCode::Enter => {
                let at = self.byte(self.col);
                let rest = self.lines[self.row].split_off(at);
                self.lines.insert(self.row + 1, rest);
                self.row += 1;
                self.col = 0;
            }
            KeyCode::Backspace => {
                if self.col > 0 {
                    let at = self.byte(self.col - 1);
                    self.lines[self.row].remove(at);
                    self.col -= 1;
                } else if self.row > 0 {
                    let cur = self.lines.remove(self.row);
                    self.row -= 1;
                    self.col = self.len();
                    self.lines[self.row].push_str(&cur);
                }
            }
            KeyCode::Delete => {
                if self.col < self.len() {
                    let at = self.byte(self.col);
                    self.lines[self.row].remove(at);
                } else if self.row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.row + 1);
                    self.lines[self.row].push_str(&next);
                }
            }
            KeyCode::Left => {
                if self.col > 0 {
                    self.col -= 1;
                } else if self.row > 0 {
                    self.row -= 1;
                    self.col = self.len();
                }
            }
            KeyCode::Right => {
                if self.col < self.len() {
                    self.col += 1;
                } else if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = 0;
                }
            }
            KeyCode::Up => {
                if self.row > 0 {
                    self.row -= 1;
                    self.col = self.col.min(self.len());
                }
            }
            KeyCode::Down => {
                if self.row + 1 < self.lines.len() {
                    self.row += 1;
                    self.col = self.col.min(self.len());
                }
            }
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.len(),
            KeyCode::Char(c) if !ctrl || alt => {
                // (AltGr arrives as Ctrl+Alt and is a plain character)
                let at = self.byte(self.col);
                self.lines[self.row].insert(at, c);
                self.col += 1;
            }
            _ => {}
        }
        Outcome::Continue
    }
}

/// CLI: read a multi-line text below the current line. Returns `None` if cancelled. Piped input is read to the end.
pub fn run_inline(title: &str, initial: &str) -> io::Result<Option<String>> {
    if !io::stdin().is_terminal() {
        let mut s = String::new();
        io::Read::read_to_string(&mut io::stdin(), &mut s)?;
        return Ok(Some(s));
    }
    let mut ta = TextArea::new(initial);
    let mut out = io::stdout();
    println!("{title}");
    println!("Enter = new line, Ctrl+Enter (or Ctrl+S) = save, Esc = cancel");
    terminal::enable_raw_mode()?;
    let mut shown = 0usize; // lines drawn so far
    let res = (|| -> io::Result<Option<String>> {
        loop {
            let (cols, _) = terminal::size().unwrap_or((80, 24));
            let cols = cols as usize;
            // go back to the first drawn line, clear, redraw
            if shown > 1 {
                queue!(out, MoveUp(shown as u16 - 1))?;
            }
            queue!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
            for (i, l) in ta.lines.iter().enumerate() {
                let cut: String = l.chars().take(cols.saturating_sub(3)).collect();
                if i > 0 {
                    queue!(out, Print("\r\n"))?;
                }
                queue!(out, Print(format!("| {cut}")))?;
            }
            shown = ta.lines.len();
            let up = ta.lines.len() - 1 - ta.row;
            let before: String = ta.lines[ta.row].chars().take(ta.col).collect();
            if up > 0 {
                queue!(out, MoveUp(up as u16))?;
            }
            queue!(out, MoveToColumn((2 + before.width()).min(cols.saturating_sub(1)) as u16))?;
            out.flush()?;
            let Event::Key(k) = event::read()? else { continue };
            if k.kind == KeyEventKind::Release {
                continue;
            }
            match ta.handle_key(k) {
                Outcome::Continue => {}
                Outcome::Save => return Ok(Some(ta.text())),
                Outcome::Cancel => return Ok(None),
            }
        }
    })();
    // leave the cursor below the text
    let below = ta.lines.len() - 1 - ta.row;
    if below > 0 {
        queue!(out, MoveDown(below as u16))?;
    }
    out.flush()?;
    terminal::disable_raw_mode()?;
    println!();
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyEventState;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent { code, modifiers: m, kind: KeyEventKind::Press, state: KeyEventState::NONE }
    }

    #[test]
    fn typing_lines_and_saving() {
        let mut ta = TextArea::new("");
        for c in "ab".chars() {
            ta.handle_key(key(KeyCode::Char(c), KeyModifiers::NONE));
        }
        ta.handle_key(key(KeyCode::Enter, KeyModifiers::NONE));
        ta.handle_key(key(KeyCode::Char('c'), KeyModifiers::NONE));
        assert_eq!(ta.text(), "ab\nc\n");
        ta.handle_key(key(KeyCode::Left, KeyModifiers::NONE));
        ta.handle_key(key(KeyCode::Backspace, KeyModifiers::NONE)); // join
        assert_eq!(ta.text(), "abc\n");
        assert_eq!(ta.handle_key(key(KeyCode::Enter, KeyModifiers::CONTROL)), Outcome::Save);
        assert_eq!(ta.handle_key(key(KeyCode::Esc, KeyModifiers::NONE)), Outcome::Cancel);
    }

    #[test]
    fn existing_text_trailing_newline() {
        let ta = TextArea::new("one\ntwo\n");
        assert_eq!(ta.lines, vec!["one", "two"]);
        assert_eq!((ta.row, ta.col), (1, 3));
    }
}

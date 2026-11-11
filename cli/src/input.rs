//! A single-line text input with its own undo/redo, whole-word removal (Ctrl+Backspace/Delete)
//! and automatic `due:` date conversion. Used by the terminal UI and by `todo --edit`.
//! Behaviour mirrors web/composer.js.

use crate::todotxt;
use chrono::NaiveDate;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What `LineInput::handle_key` did with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edited {
    /// Not an editing key.
    No,
    /// Only the cursor moved.
    Moved,
    /// The text changed.
    Yes,
    /// A whole word was removed (see `WordDel`).
    Word(WordDel),
}

#[derive(Clone)]
struct Snap {
    text: String,
    cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordDel {
    Removed,
    /// Only the name of a `+tag` was cleared; a bare `+` is left.
    Project,
    /// Only the name of an `@context` was cleared; a bare `@` is left.
    Context,
}

pub struct LineInput {
    pub text: String,
    /// Cursor position in characters.
    pub cursor: usize,
    stack: Vec<Snap>,
    pos: usize,
    typing: bool,
}

impl LineInput {
    pub fn new(text: &str) -> Self {
        let n = text.chars().count();
        LineInput {
            text: text.to_string(),
            cursor: n,
            stack: vec![Snap { text: text.to_string(), cursor: n }],
            pos: 0,
            typing: false,
        }
    }

    /// Replace the content and forget the history.
    pub fn set(&mut self, text: &str) {
        *self = LineInput::new(text);
    }

    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    fn byte(&self, c: usize) -> usize {
        self.text.char_indices().nth(c).map_or(self.text.len(), |(i, _)| i)
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    /// Remember the caret of the current state, so undo puts it back where it was.
    fn checkpoint(&mut self) {
        let (text, cursor) = (self.text.clone(), self.cursor);
        if let Some(top) = self.stack.get_mut(self.pos) {
            if top.text == text {
                top.cursor = cursor;
            }
        }
    }

    fn snap(&mut self, coalesce: bool) {
        let s = Snap { text: self.text.clone(), cursor: self.cursor };
        if coalesce && self.typing && self.pos + 1 == self.stack.len() && self.pos > 0 {
            self.stack[self.pos] = s; // consecutive typing is one undo step
        } else {
            self.stack.truncate(self.pos + 1);
            self.stack.push(s);
            if self.stack.len() > 200 {
                self.stack.remove(0);
            }
            self.pos = self.stack.len() - 1;
        }
        self.typing = coalesce;
    }

    fn set_text(&mut self, text: String, cursor: usize) {
        self.text = text;
        self.cursor = cursor.min(self.len());
    }

    // ---- editing ----

    pub fn insert(&mut self, ch: char) {
        self.checkpoint();
        let b = self.byte(self.cursor);
        self.text.insert(b, ch);
        self.cursor += 1;
        self.snap(!ch.is_whitespace());
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.checkpoint();
        let b = self.byte(self.cursor - 1);
        self.text.remove(b);
        self.cursor -= 1;
        self.snap(false);
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.len() {
            return;
        }
        self.checkpoint();
        let b = self.byte(self.cursor);
        self.text.remove(b);
        self.snap(false);
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.typing = false;
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.len());
        self.typing = false;
    }

    pub fn home(&mut self) {
        self.cursor = 0;
        self.typing = false;
    }

    pub fn end(&mut self) {
        self.cursor = self.len();
        self.typing = false;
    }

    pub fn word_left(&mut self) {
        let c = self.chars();
        let mut i = self.cursor;
        while i > 0 && c[i - 1].is_whitespace() {
            i -= 1;
        }
        while i > 0 && !c[i - 1].is_whitespace() {
            i -= 1;
        }
        self.cursor = i;
        self.typing = false;
    }

    pub fn word_right(&mut self) {
        let c = self.chars();
        let mut i = self.cursor;
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        while i < c.len() && !c[i].is_whitespace() {
            i += 1;
        }
        self.cursor = i;
        self.typing = false;
    }

    /// Ctrl+Backspace (`dir < 0`) / Ctrl+Delete (`dir > 0`): remove the whole word under or next to
    /// the cursor. For a `+tag` / `@context` only the name is cleared (so a new one can be chosen);
    /// a second press removes the bare marker.
    pub fn delete_word(&mut self, dir: i32) -> Option<WordDel> {
        let c = self.chars();
        let n = c.len();
        let sp = |k: usize| c[k].is_whitespace();
        let i: usize = if dir < 0 {
            let mut i = self.cursor;
            while i > 0 && sp(i - 1) {
                i -= 1;
            }
            if i == 0 {
                return None;
            }
            i - 1
        } else {
            let mut i = self.cursor;
            while i < n && sp(i) {
                i += 1;
            }
            if i >= n {
                return None;
            }
            i
        };
        let mut start = i;
        let mut end = i + 1;
        while start > 0 && !sp(start - 1) {
            start -= 1;
        }
        while end < n && !sp(end) {
            end += 1;
        }
        let token = &c[start..end];

        let (mut from, mut to) = (start, end);
        let mut keep: Option<char> = None;
        let mut result = WordDel::Removed;
        if token.len() > 1 && (token[0] == '+' || token[0] == '@') {
            keep = Some(token[0]);
            result = if token[0] == '+' { WordDel::Project } else { WordDel::Context };
        } else {
            let mut k = end;
            while k < n && sp(k) {
                k += 1;
            }
            if k > end {
                to = k; // also drop the following spaces
            } else {
                while from > 0 && sp(from - 1) {
                    from -= 1; // last word: drop the preceding spaces
                }
            }
        }
        self.checkpoint();
        let mut out: String = c[..from].iter().collect();
        if let Some(k) = keep {
            out.push(k);
        }
        out.extend(c[to..].iter());
        let cursor = from + keep.map_or(0, |_| 1);
        self.set_text(out, cursor);
        self.snap(false);
        Some(result)
    }

    /// The word around the cursor: (start, end, text), positions in characters.
    pub fn word_at_cursor(&self) -> (usize, usize, String) {
        let c = self.chars();
        let mut start = self.cursor.min(c.len());
        while start > 0 && !c[start - 1].is_whitespace() {
            start -= 1;
        }
        let mut end = self.cursor.min(c.len());
        while end < c.len() && !c[end].is_whitespace() {
            end += 1;
        }
        (start, end, c[start..end].iter().collect())
    }

    /// Replace characters [start, end) with `text` plus a space, as one undo step.
    pub fn replace_range(&mut self, start: usize, end: usize, text: &str) {
        self.checkpoint();
        let c = self.chars();
        let end = end.min(c.len());
        let start = start.min(end);
        let after: String = c[end..].iter().collect();
        let pad = if after.starts_with(char::is_whitespace) { "" } else { " " };
        let out = format!("{}{}{}{}", c[..start].iter().collect::<String>(), text, pad, after);
        let cursor = start + text.chars().count() + 1;
        self.set_text(out, cursor);
        self.snap(false);
    }

    /// After a space was typed: turn `due:tomorrow`, `due:15.03.`, `due:+3d` ... before the cursor
    /// into `due:YYYY-MM-DD` (its own undo step). Returns true if something changed.
    pub fn auto_date(&mut self, today: NaiveDate) -> bool {
        let c = self.chars();
        let before: String = c[..self.cursor.min(c.len())].iter().collect();
        let fixed = todotxt::normalize_due(&before, today);
        if fixed == before {
            return false;
        }
        let after: String = c[self.cursor.min(c.len())..].iter().collect();
        let cursor = fixed.chars().count();
        self.set_text(format!("{fixed}{after}"), cursor);
        self.snap(false);
        true
    }

    /// True when the cursor is right behind a complete word `note:` (just typed).
    pub fn note_trigger(&self) -> bool {
        let (_, end, word) = self.word_at_cursor();
        word == "note:" && end == self.cursor
    }

    /// Finish the `note:` word at the cursor: `Some(name)` makes it `note:NAME`, `None` removes it.
    pub fn set_note_word(&mut self, name: Option<&str>) {
        let (start, end, word) = self.word_at_cursor();
        if word != "note:" {
            return;
        }
        match name {
            Some(n) => self.replace_range(start, end, &format!("note:{n}")),
            None => {
                self.checkpoint();
                let c = self.chars();
                let mut end = end;
                if end < c.len() && c[end].is_whitespace() {
                    end += 1;
                }
                let out: String = c[..start].iter().chain(c[end..].iter()).collect();
                self.set_text(out, start);
                self.snap(false);
            }
        }
    }

    /// `(a) ` at the very start becomes `(A) `. Returns true if something changed.
    pub fn fix_priority_case(&mut self) -> bool {
        let fixed = todotxt::fix_priority_case(&self.text);
        if fixed == self.text {
            return false;
        }
        self.text = fixed;
        self.snap(false);
        true
    }

    /// Handle a plain editing key. AltGr (reported as Ctrl+Alt, e.g. `@` on a German keyboard)
    /// counts as a normal character.
    pub fn handle_key(&mut self, k: KeyEvent, today: NaiveDate) -> Edited {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let ctrl_only = ctrl && !alt;
        match k.code {
            KeyCode::Char('z') if ctrl_only => {
                self.undo();
                Edited::Yes
            }
            KeyCode::Char('y') if ctrl_only => {
                self.redo();
                Edited::Yes
            }
            KeyCode::Char('h') | KeyCode::Char('w') if ctrl_only => self.word_removed(-1),
            KeyCode::Backspace if ctrl || alt => self.word_removed(-1),
            KeyCode::Delete if ctrl => self.word_removed(1),
            KeyCode::Backspace => {
                self.backspace();
                Edited::Yes
            }
            KeyCode::Delete => {
                self.delete();
                Edited::Yes
            }
            KeyCode::Left if ctrl => {
                self.word_left();
                Edited::Moved
            }
            KeyCode::Right if ctrl => {
                self.word_right();
                Edited::Moved
            }
            KeyCode::Left => {
                self.left();
                Edited::Moved
            }
            KeyCode::Right => {
                self.right();
                Edited::Moved
            }
            KeyCode::Home => {
                self.home();
                Edited::Moved
            }
            KeyCode::End => {
                self.end();
                Edited::Moved
            }
            KeyCode::Char(c) if !ctrl && !alt || (ctrl && alt) => {
                self.insert(c);
                self.fix_priority_case();
                if c.is_whitespace() {
                    self.auto_date(today);
                }
                Edited::Yes
            }
            _ => Edited::No,
        }
    }

    fn word_removed(&mut self, dir: i32) -> Edited {
        match self.delete_word(dir) {
            Some(w) => Edited::Word(w),
            None => Edited::Yes,
        }
    }

    // ---- undo / redo ----

    pub fn undo(&mut self) {
        self.checkpoint();
        if self.pos > 0 {
            self.pos -= 1;
            let s = self.stack[self.pos].clone();
            self.set_text(s.text, s.cursor);
        }
        self.typing = false;
    }

    pub fn redo(&mut self) {
        if self.pos + 1 < self.stack.len() {
            self.pos += 1;
            let s = self.stack[self.pos].clone();
            self.set_text(s.text, s.cursor);
        }
        self.typing = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> LineInput {
        let mut i = LineInput::new("");
        for ch in s.chars() {
            i.insert(ch);
        }
        i
    }

    #[test]
    fn typing_is_one_undo_step_per_word() {
        let mut i = typed("Call Mom");
        i.undo();
        assert_eq!(i.text, "Call ");
        i.undo();
        assert_eq!(i.text, "Call");
        i.undo();
        assert_eq!(i.text, "");
        i.redo();
        assert_eq!(i.text, "Call");
    }

    #[test]
    fn word_removal_and_undo() {
        let mut i = typed("Call Mom +Family @phone due:2026-12-01");
        assert_eq!(i.delete_word(-1), Some(WordDel::Removed));
        assert_eq!(i.text, "Call Mom +Family @phone");
        assert_eq!(i.delete_word(-1), Some(WordDel::Context));
        assert_eq!(i.text, "Call Mom +Family @");
        assert_eq!(i.delete_word(-1), Some(WordDel::Removed));
        assert_eq!(i.text, "Call Mom +Family");
        i.undo();
        assert_eq!(i.text, "Call Mom +Family @");
        i.undo();
        assert_eq!(i.text, "Call Mom +Family @phone");
        i.undo();
        assert_eq!(i.text, "Call Mom +Family @phone due:2026-12-01");
        assert_eq!(i.cursor, i.len());
    }

    #[test]
    fn delete_whole_word_from_inside() {
        let mut i = LineInput::new("aa bb cc");
        i.cursor = 4;
        assert_eq!(i.delete_word(1), Some(WordDel::Removed));
        assert_eq!(i.text, "aa cc");
    }

    #[test]
    fn auto_date_on_space() {
        let today = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();
        let mut i = typed("Buy due:tomorrow ");
        assert!(i.auto_date(today));
        assert_eq!(i.text, "Buy due:2026-11-11 ");
        i.undo();
        assert_eq!(i.text, "Buy due:tomorrow ");
    }

    #[test]
    fn priority_case() {
        let mut i = typed("(a)");
        assert!(i.fix_priority_case() || i.text == "(A)");
        assert_eq!(i.text, "(A)");
        assert_eq!(todotxt::fix_priority_case("x (a)"), "x (a)");
    }

    #[test]
    fn replace_range_adds_space() {
        let mut i = typed("Buy +Fa");
        let (s, e, w) = i.word_at_cursor();
        assert_eq!(w, "+Fa");
        i.replace_range(s, e, "+Family");
        assert_eq!(i.text, "Buy +Family ");
    }
}

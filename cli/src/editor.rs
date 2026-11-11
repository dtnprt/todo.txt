//! `todo --edit N`: an interactive one-line editor with the current text pre-filled.
//! Enter saves, Esc cancels. Supports the same keys as the TUI input (Ctrl+Backspace removes a
//! whole word, Ctrl+Z / Ctrl+Y undo / redo, `due:tomorrow` -> date on space, ...).

use crate::folder::{self, NoteCtx};
use crate::highlight::{highlight};
use crate::input::LineInput;
use crate::ops::paint;
use crate::todotxt;
use ratatui::crossterm::{
    cursor::MoveToColumn,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};
use std::io::{self, IsTerminal, Write};
use unicode_width::UnicodeWidthStr;

/// Returns `Ok(None)` if the user cancelled.
pub fn edit_line(prompt: &str, initial: &str, color: bool, notes: Option<&NoteCtx>) -> io::Result<Option<String>> {
    if !io::stdin().is_terminal() {
        // piped input: take one line as the new text
        let mut s = String::new();
        io::stdin().read_line(&mut s)?;
        return Ok(Some(s.trim_end_matches(['\r', '\n']).to_string()));
    }
    let mut input = LineInput::new(initial);
    terminal::enable_raw_mode()?;
    let result = run(prompt, &mut input, color, notes);
    terminal::disable_raw_mode()?;
    println!();
    result
}

fn run(prompt: &str, input: &mut LineInput, color: bool, notes: Option<&NoteCtx>) -> io::Result<Option<String>> {
    let mut out = io::stdout();
    let today = todotxt::today();
    loop {
        draw(&mut out, prompt, input, color)?;
        let Event::Key(k) = event::read()? else { continue };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Enter => return Ok(Some(input.text.clone())),
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') | KeyCode::Char('q') if ctrl => return Ok(None),
            _ => {
                input.handle_key(k, today);
                if let (KeyCode::Char(':'), Some(ctx)) = (k.code, notes) {
                    if input.note_trigger() {
                        // like the GUI: a note editor opens after typing `note:`
                        let name = folder::next_note_name_with(&ctx.lines, &input.text, ctx.folder);
                        terminal::disable_raw_mode()?;
                        println!();
                        let res = folder::note_via_editor(ctx.todo, ctx.folder, &name);
                        terminal::enable_raw_mode()?;
                        match res {
                            Ok(true) => input.set_note_word(Some(&name)),
                            Ok(false) => input.set_note_word(None),
                            Err(_) => input.set_note_word(None),
                        }
                    }
                }
            }
        }
    }
}

/// Keep the cursor visible: returns the first character to show.
fn window_start(text: &str, cursor: usize, room: usize) -> usize {
    let mut start = 0;
    while start < cursor {
        let w: usize = text.chars().skip(start).take(cursor - start).collect::<String>().width();
        if w < room {
            break;
        }
        start += 1;
    }
    start
}

fn draw(out: &mut impl Write, prompt: &str, input: &LineInput, color: bool) -> io::Result<()> {
    let (cols, _) = terminal::size().unwrap_or((80, 24));
    let room = (cols as usize).saturating_sub(prompt.width() + 1).max(4);
    let start = window_start(&input.text, input.cursor, room);

    // visible slice, cut to the available width
    let mut visible = String::new();
    let mut used = 0;
    for ch in input.text.chars().skip(start) {
        let w = ch.to_string().width();
        if used + w > room {
            break;
        }
        used += w;
        visible.push(ch);
    }

    let mut line = String::from(prompt);
    if color {
        for (piece, tok) in highlight(&visible, start == 0) {
            line.push_str(&paint(&piece, tok, "", true));
        }
    } else {
        line.push_str(&visible);
    }
    let cursor_col = prompt.width() + input.text.chars().skip(start).take(input.cursor - start).collect::<String>().width();
    queue!(out, MoveToColumn(0), Clear(ClearType::CurrentLine), Print(line), MoveToColumn(cursor_col as u16))?;
    out.flush()
}

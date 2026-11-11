//! Non-interactive actions: listing, adding, changing, deleting.

use crate::highlight::{highlight, Tok};
use crate::list::{self, Filter, Sort};
use crate::todotxt::{self, DueStatus};
use std::io::IsTerminal;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Coloured list with line numbers and checkboxes (`--list`).
    Pretty,
    /// The raw lines with their line numbers (`--text`).
    Numbered,
    /// Only the raw text (`--plain`).
    Plain,
}

/// ANSI colours only on a terminal (and not with NO_COLOR).
pub fn use_color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Make ANSI escapes work in the classic Windows console.
pub fn enable_ansi() {
    #[cfg(windows)]
    {
        let _ = ratatui::crossterm::ansi_support::supports_ansi();
    }
}

fn pri_code(c: char) -> &'static str {
    match c {
        'A' => "1;31",
        'B' => "1;38;5;208",
        'C' => "1;33",
        'D' => "1;32",
        _ => "90",
    }
}

pub fn paint(s: &str, tok: Tok, today: &str, color: bool) -> String {
    if !color {
        return s.to_string();
    }
    let code = match tok {
        Tok::Plain => return s.to_string(),
        Tok::Priority(c) => pri_code(c),
        Tok::Done => "32",
        Tok::Date | Tok::Meta => "90",
        Tok::Code => "36",
        Tok::Project => "94",
        Tok::Context => "32",
        Tok::Due => match s.strip_prefix("due:").and_then(|d| {
            let mut t = todotxt::Task::default();
            t.tags.insert("due".into(), d.to_string());
            todotxt::due_status(&t, today)
        }) {
            Some(st) => due_code(st),
            None => "33",
        },
    };
    format!("\x1b[{code}m{s}\x1b[0m")
}

/// ANSI code for a due state: soon = yellow, today = orange, overdue = red.
fn due_code(st: DueStatus) -> &'static str {
    match st {
        DueStatus::Overdue => "1;31",
        DueStatus::Today => "1;38;5;208",
        DueStatus::Soon => "1;33",
        DueStatus::Upcoming => "90",
    }
}

fn dim(s: &str, color: bool) -> String {
    if color { format!("\x1b[90m{s}\x1b[0m") } else { s.to_string() }
}

pub fn print_list(lines: &[String], sort: Sort, style: Style, file_name: &str, header: &str, folder: Option<&std::path::Path>, filter: &Filter, show_done: bool, verbose: bool) {
    let color = use_color();
    let today = todotxt::iso(todotxt::today());
    let order = list::view(lines, sort, show_done || style != Style::Pretty, filter);
    if style == Style::Pretty {
        // short file name, then a thin rule as long as the name (at least 12 characters)
        let n = header.chars().count().clamp(12, 60);
        println!("{}", dim(header, color));
        println!("{}", dim(&"─".repeat(n), color));
    }
    for &i in &order {
        let raw = &lines[i];
        match style {
            Style::Plain => println!("{raw}"),
            Style::Numbered => println!("{:>3}  {raw}", i + 1),
            Style::Pretty => {
                let t = todotxt::parse(raw);
                // the line number carries the due colour (yellow soon, orange today, red overdue)
                let num = format!("{:>3} ", i + 1);
                let mut row = match todotxt::due_status(&t, &today).filter(|_| !t.done) {
                    Some(st) if color && st != DueStatus::Upcoming => format!("\x1b[{}m{num}\x1b[0m", due_code(st)),
                    _ => dim(&num, color),
                };
                row.push_str(if t.done { "[x] " } else { "[ ] " });
                match t.priority {
                    Some(p) => row.push_str(&paint(&format!("({p}) "), Tok::Priority(p), &today, color && !t.done)),
                    None => row.push_str("    "),
                }
                for (piece, tok) in highlight(&t.text, false) {
                    if t.done {
                        row.push_str(&dim(&piece, color));
                    } else {
                        row.push_str(&paint(&piece, tok, &today, color));
                    }
                }
                println!("{row}");
            }
        }
    }
    if style == Style::Pretty && verbose {
        let open = order.iter().filter(|&&i| !todotxt::parse(&lines[i]).done).count();
        let place = match folder {
            Some(d) => format!("{file_name} in {}", d.display()),
            None => file_name.to_string(),
        };
        println!("{}", dim(&format!("-- {open} open, {} total ({place})", order.len()), color));
    }
}

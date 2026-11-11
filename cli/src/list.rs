//! Sorting, filtering and counting tasks (shared by the command line and the terminal UI).

use crate::todotxt::{self, Task};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sort {
    Priority,
    Due,
    Created,
    Alpha,
    File,
}

impl Sort {
    pub const ALL: [Sort; 5] = [Sort::Priority, Sort::Due, Sort::Created, Sort::Alpha, Sort::File];

    pub fn parse(s: &str) -> Option<Sort> {
        Some(match s.trim().to_lowercase().as_str() {
            "prio" | "pri" | "priority" | "p" => Sort::Priority,
            "due" | "date" | "data" | "d" => Sort::Due,
            "created" | "create" | "c" => Sort::Created,
            "alpha" | "alphabetical" | "az" | "a" | "name" | "text" => Sort::Alpha,
            "file" | "none" | "order" | "f" => Sort::File,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Sort::Priority => "priority",
            Sort::Due => "due",
            Sort::Created => "created",
            Sort::Alpha => "alpha",
            Sort::File => "file",
        }
    }

    pub fn next(self) -> Sort {
        let i = Sort::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Sort::ALL[(i + 1) % Sort::ALL.len()]
    }
}

#[derive(Default, Clone, Debug)]
pub struct Filter {
    pub q: String,
    pub project: Option<String>,
    pub context: Option<String>,
    pub priority: Option<char>,
    /// Extra terms from a command-line filter expression; all must match.
    pub terms: Vec<String>,
}

impl Filter {
    /// Build a filter from an expression like `"+Home @phone (A) text"`:
    /// `+x` project, `@x` context, `(A)` priority, anything else = case-insensitive substring.
    /// Every term must match (AND).
    pub fn from_expr(expr: &str) -> Filter {
        Filter { terms: expr.split_whitespace().map(str::to_string).collect(), ..Filter::default() }
    }

    pub fn is_active(&self) -> bool {
        !self.q.trim().is_empty()
            || self.project.is_some()
            || self.context.is_some()
            || self.priority.is_some()
            || !self.terms.is_empty()
    }

    fn term_matches(term: &str, raw: &str, t: &Task) -> bool {
        let ci = |list: &[String], name: &str| list.iter().any(|x| x.eq_ignore_ascii_case(name));
        let b = term.as_bytes();
        if term.len() > 1 && b[0] == b'+' {
            ci(&t.projects, &term[1..])
        } else if term.len() > 1 && b[0] == b'@' {
            ci(&t.contexts, &term[1..])
        } else if term.len() == 3 && b[0] == b'(' && b[2] == b')' && b[1].is_ascii_alphabetic() {
            t.priority.map(|p| p.to_ascii_uppercase()) == Some((b[1] as char).to_ascii_uppercase())
        } else {
            raw.to_lowercase().contains(&term.to_lowercase())
        }
    }

    pub fn clear(&mut self) {
        *self = Filter::default();
    }

    fn matches(&self, raw: &str, t: &Task) -> bool {
        if let Some(p) = &self.project {
            if !t.projects.contains(p) {
                return false;
            }
        }
        if let Some(c) = &self.context {
            if !t.contexts.contains(c) {
                return false;
            }
        }
        if let Some(p) = self.priority {
            if t.priority != Some(p) {
                return false;
            }
        }
        if !self.terms.iter().all(|term| Filter::term_matches(term, raw, t)) {
            return false;
        }
        let q = self.q.trim().to_lowercase();
        q.is_empty() || raw.to_lowercase().contains(&q)
    }
}

/// Line indices to show, in display order (blank lines never show up).
pub fn view(lines: &[String], sort: Sort, show_done: bool, filter: &Filter) -> Vec<usize> {
    let mut items: Vec<(usize, Task)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| (i, todotxt::parse(l)))
        .filter(|(_, t)| (show_done || !t.done) && filter.matches(&t.raw, t))
        .collect();

    // Same ordering as the web app: completed tasks last, then the key, then file order.
    let last = "\u{ffff}".to_string();
    match sort {
        Sort::File => {}
        Sort::Priority => items.sort_by_key(|(i, t)| (t.done, t.priority.map_or(u8::MAX, |c| c as u8), *i)),
        Sort::Due => items.sort_by(|(ia, a), (ib, b)| {
            (a.done, a.tags.get("due").unwrap_or(&last), ia).cmp(&(b.done, b.tags.get("due").unwrap_or(&last), ib))
        }),
        Sort::Created => items.sort_by(|(ia, a), (ib, b)| {
            (a.done, a.created.as_ref().unwrap_or(&last), ia).cmp(&(b.done, b.created.as_ref().unwrap_or(&last), ib))
        }),
        Sort::Alpha => items.sort_by_cached_key(|(i, t)| (t.done, t.text.to_lowercase(), *i)),
    }
    items.into_iter().map(|(i, _)| i).collect()
}

/// (name, count) pairs, alphabetical. `pick` returns the names a task contributes.
pub fn counts<F>(lines: &[String], include_done: bool, pick: F) -> Vec<(String, usize)>
where
    F: Fn(&Task) -> Vec<String>,
{
    let mut m: BTreeMap<String, usize> = BTreeMap::new();
    for l in lines.iter().filter(|l| !l.trim().is_empty()) {
        let t = todotxt::parse(l);
        if t.done && !include_done {
            continue;
        }
        let mut names = pick(&t);
        names.sort();
        names.dedup();
        for n in names {
            *m.entry(n).or_insert(0) += 1;
        }
    }
    m.into_iter().collect()
}

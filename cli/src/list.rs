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
    /// Sidebar selections: several entries of one section match with OR, sections combine with AND.
    pub project: Vec<String>,
    pub context: Vec<String>,
    pub priority: Vec<char>,
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
            || !self.project.is_empty()
            || !self.context.is_empty()
            || !self.priority.is_empty()
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

    /// Does the line match every term (`+x`, `@x`, `(A)` or a substring)?
    pub fn terms_match(terms: &[String], raw: &str) -> bool {
        let t = todotxt::parse(raw);
        terms.iter().all(|term| Filter::term_matches(term, raw, &t))
    }

    pub fn clear(&mut self) {
        *self = Filter::default();
    }

    /// Does the line pass this filter (sidebar selections + search text + terms)?
    pub fn matches_line(&self, raw: &str) -> bool {
        self.matches(raw, &todotxt::parse(raw))
    }

    fn matches(&self, raw: &str, t: &Task) -> bool {
        if !self.project.is_empty() && !self.project.iter().any(|p| t.projects.contains(p)) {
            return false;
        }
        if !self.context.is_empty() && !self.context.iter().any(|c| t.contexts.contains(c)) {
            return false;
        }
        if !self.priority.is_empty() && !t.priority.map_or(false, |p| self.priority.contains(&p)) {
            return false;
        }
        if !self.terms.iter().all(|term| Filter::term_matches(term, raw, t)) {
            return false;
        }
        let q = self.q.trim().to_lowercase();
        q.is_empty() || raw.to_lowercase().contains(&q)
    }
}

/// Line indices to show, in display order (blank lines never show up).
/// Sub-entries (`- `, `-- `) stay below their top-level entry: groups are sorted by the top-level entry,
/// inside a group the file order is kept.
pub fn view(lines: &[String], sort: Sort, show_done: bool, filter: &Filter) -> Vec<usize> {
    let heads = todotxt::group_heads(lines);
    let parsed: Vec<Option<Task>> = lines.iter().map(|l| if l.trim().is_empty() { None } else { Some(todotxt::parse(l)) }).collect();
    let mut items: Vec<(usize, &Task)> = parsed
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.as_ref().map(|t| (i, t)))
        .filter(|(_, t)| (show_done || !t.done) && filter.matches(&t.raw, t))
        .collect();
    // the entry a line is sorted by: the head of its group
    let head = |i: usize| -> (usize, &Task) {
        let h = heads[i];
        (h, parsed[h].as_ref().unwrap_or_else(|| parsed[i].as_ref().unwrap()))
    };

    // Same ordering as the web app: completed tasks last, then the key, then file order.
    let last = "\u{ffff}".to_string();
    match sort {
        Sort::File => {}
        Sort::Priority => items.sort_by_key(|&(i, _)| {
            let (h, t) = head(i);
            (t.done, t.priority.map_or(u8::MAX, |c| c as u8), h, i)
        }),
        Sort::Due => items.sort_by(|&(ia, _), &(ib, _)| {
            let ((ha, a), (hb, b)) = (head(ia), head(ib));
            (a.done, a.tags.get("due").unwrap_or(&last), ha, ia).cmp(&(b.done, b.tags.get("due").unwrap_or(&last), hb, ib))
        }),
        Sort::Created => items.sort_by(|&(ia, _), &(ib, _)| {
            let ((ha, a), (hb, b)) = (head(ia), head(ib));
            (a.done, a.created.as_ref().unwrap_or(&last), ha, ia).cmp(&(b.done, b.created.as_ref().unwrap_or(&last), hb, ib))
        }),
        Sort::Alpha => items.sort_by_cached_key(|&(i, _)| {
            let (h, t) = head(i);
            (t.done, t.text.to_lowercase(), h, i)
        }),
    }
    items.into_iter().map(|(i, _)| i).collect()
}

/// Lines to move for a filter expression (terms, all must match): see `select_matching`.
pub fn select_for_move(lines: &[String], terms: &[String]) -> Vec<usize> {
    if terms.is_empty() {
        return Vec::new();
    }
    select_matching(lines, |l| Filter::terms_match(terms, l))
}

/// Every line `matches` accepts together with its sub-entries (blank lines are skipped).
pub fn select_matching<F: Fn(&str) -> bool>(lines: &[String], matches: F) -> Vec<usize> {
    let mut picked = std::collections::BTreeSet::new();
    for (i, l) in lines.iter().enumerate() {
        if l.trim().is_empty() || picked.contains(&i) || !matches(l) {
            continue;
        }
        let end = todotxt::subtree_end(lines, i);
        picked.extend((i..=end).filter(|&j| !lines[j].trim().is_empty()));
    }
    picked.into_iter().collect()
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

/// Lines removed from / added to a list between two versions (order-independent).
#[allow(unused_imports)]
pub use crate::davcache::{line_diff, reordered};

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sub_entries_stay_under_parent() {
        let l = lines(&["(B) b +x", "- sub of b", "(A) a", "-- subsub of a? (orphan level 2)", "(C) c"]);
        // priority sort: A group first, then B group, then C; children keep file order below their parent
        let v = view(&l, Sort::Priority, true, &Filter::default());
        assert_eq!(v, vec![2, 3, 0, 1, 4]);
        let v = view(&l, Sort::Alpha, true, &Filter::default());
        assert_eq!(v, vec![2, 3, 0, 1, 4]);
        // a filter keeps the order and only shows the matching lines
        let v = view(&l, Sort::Priority, true, &Filter::from_expr("sub"));
        assert_eq!(v, vec![3, 1]);
    }

    #[test]
    fn move_selection_takes_the_sub_tree() {
        let l = lines(&["a +p", "- a1", "-- a11", "b", "- b1 +p", "c +p", "", "- c1"]);
        let terms = vec!["+p".to_string()];
        assert_eq!(select_for_move(&l, &terms), vec![0, 1, 2, 4, 5, 7]);
        assert!(select_for_move(&l, &[]).is_empty());
    }
}

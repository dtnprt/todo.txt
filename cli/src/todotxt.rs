//! todo.txt format logic. This is a port of `web/todotxt.js`; keep both in sync.
//! Spec: ../../description/README.md

use chrono::{Months, NaiveDate};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// Deepest sub-entry level (`----`).
pub const MAX_LEVEL: usize = 4;

/// Level (1-4) of a leading `- `..`---- ` marker, and the text after it.
fn dash_prefix(s: &str) -> Option<(usize, &str)> {
    let n = s.bytes().take_while(|&b| b == b'-').count();
    ((1..=MAX_LEVEL).contains(&n) && s.as_bytes().get(n) == Some(&b' ')).then(|| (n, &s[n + 1..]))
}

/// Byte length of the head of a line: `x DONE CREATED ` or `(P) CREATED ` (each part optional).
fn head_len(s: &str) -> usize {
    let date_at = |p: usize| s.get(p..p + 11).is_some_and(|w| is_date_shape(&w[..10]) && w.ends_with(' '));
    let mut pos = 0;
    if s.starts_with("x ") {
        pos = 2;
        if date_at(pos) {
            pos += 11;
            if date_at(pos) {
                pos += 11;
            }
        }
    } else {
        if priority_prefix(s).is_some() {
            pos = 4;
        }
        if date_at(pos) {
            pos += 11;
        }
    }
    pos
}

/// Sub-entries: one to four dashes and a space between the dates and the text
/// (`(A) 2026-01-01 -- sub-sub`, `x 2026-01-02 2026-01-01 - sub`, `-- sub` without prefix).
/// Old files with the dashes in front of everything (`-- (A) text`) are still read.
/// Returns head (`x`/priority/dates incl. trailing space), level (0 = top level) and the text after the dashes.
pub fn split_parts(raw: &str) -> (&str, usize, &str) {
    let (lead, s) = dash_prefix(raw).unwrap_or((0, raw));
    let (head, rest) = s.split_at(head_len(s));
    match dash_prefix(rest) {
        Some((n, body)) => (head, n, body),
        None => (head, lead, rest),
    }
}

pub fn level_of(raw: &str) -> usize {
    split_parts(raw).1
}

/// `"-- "` for level 2, `""` for level 0.
pub fn level_prefix(n: usize) -> String {
    if n == 0 { String::new() } else { format!("{} ", "-".repeat(n.min(MAX_LEVEL))) }
}

/// The line at another nesting level (0 removes the dashes).
pub fn with_level(raw: &str, n: usize) -> String {
    let (head, _, body) = split_parts(raw);
    format!("{head}{}{body}", level_prefix(n))
}

/// Index of the last line of the sub-tree of line `i`: the lines below it with a deeper level
/// (blank lines do not end it).
pub fn subtree_end(lines: &[String], i: usize) -> usize {
    let lv = level_of(&lines[i]);
    let mut end = i;
    for (j, l) in lines.iter().enumerate().skip(i + 1) {
        if l.trim().is_empty() {
            continue;
        }
        if level_of(l) <= lv {
            break;
        }
        end = j;
    }
    end
}

/// For every line the index of the top-level line its group starts with (blank lines belong to the group before).
pub fn group_heads(lines: &[String]) -> Vec<usize> {
    let mut head = 0;
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 || (!l.trim().is_empty() && level_of(l) == 0) {
                head = i;
            }
            head
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
pub struct Task {
    pub raw: String,
    /// Sub-entry level: 0 = normal, 1 = `- `, ... 4 = `---- `.
    pub level: usize,
    pub done: bool,
    pub priority: Option<char>,
    pub completed: Option<String>,
    pub created: Option<String>,
    /// Description without priority, `x` and the leading dates.
    pub text: String,
    pub projects: Vec<String>,
    pub contexts: Vec<String>,
    pub tags: BTreeMap<String, String>,
}

fn is_date_shape(w: &str) -> bool {
    let b = w.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// First word and the rest, split at a single space (the format's separator).
fn split_word(s: &str) -> Option<(&str, &str)> {
    let (w, r) = s.split_once(' ')?;
    if w.is_empty() || w.contains(char::is_whitespace) {
        None
    } else {
        Some((w, r))
    }
}

fn priority_prefix(s: &str) -> Option<char> {
    let b = s.as_bytes();
    if b.len() >= 4 && b[0] == b'(' && b[1].is_ascii_uppercase() && b[2] == b')' && b[3] == b' ' {
        Some(b[1] as char)
    } else {
        None
    }
}

pub fn parse(raw: &str) -> Task {
    let (head, level, body) = split_parts(raw);
    let mut t = Task { raw: raw.to_string(), level, ..Default::default() };
    let mut toks = head.split(' ').filter(|w| !w.is_empty());
    let first = toks.next();
    if first == Some("x") {
        t.done = true;
        t.completed = toks.next().map(str::to_string);
        t.created = toks.next().map(str::to_string);
    } else {
        let mut cur = first;
        if let Some(w) = cur {
            if w.starts_with('(') {
                t.priority = w.chars().nth(1);
                cur = toks.next();
            }
        }
        t.created = cur.map(str::to_string);
    }
    let rest = body;
    t.text = rest.to_string();

    for word in rest.split_whitespace() {
        let n = word.chars().count();
        if n > 1 && word.starts_with('+') {
            t.projects.push(word[1..].to_string());
        } else if n > 1 && word.starts_with('@') {
            t.contexts.push(word[1..].to_string());
        } else if let Some((k, v)) = word.split_once(':') {
            if !k.is_empty() && !v.is_empty() && !v.contains(':') && !v.starts_with("//") {
                t.tags.insert(k.to_string(), v.to_string());
            }
        }
    }
    if t.done {
        if let Some(p) = t.tags.get("pri") {
            let mut cs = p.chars();
            if let (Some(c), None) = (cs.next(), cs.next()) {
                if c.is_ascii_uppercase() {
                    t.priority = Some(c);
                }
            }
        }
    }
    t
}

/// Mark a task complete: `x DONE CREATED text [pri:X]`.
pub fn complete(raw: &str, today: &str) -> String {
    let t = parse(raw);
    if t.done {
        return raw.to_string();
    }
    let mut out = format!("x {today} ");
    if let Some(c) = &t.created {
        out.push_str(c);
        out.push(' ');
    }
    out.push_str(&level_prefix(t.level));
    out.push_str(&t.text);
    if let Some(p) = t.priority {
        if !t.tags.contains_key("pri") {
            out.push_str(&format!(" pri:{p}"));
        }
    }
    out
}

/// Remove the first ` pri:X` token from a description.
fn remove_pri_tag(text: &str) -> String {
    for (i, _) in text.match_indices("pri:") {
        let before_ok = i == 0 || text[..i].ends_with(char::is_whitespace);
        let tail = &text[i + 4..];
        let mut cs = tail.chars();
        let letter_ok = cs.next().is_some_and(|c| c.is_ascii_uppercase());
        let after_ok = cs.next().map_or(true, char::is_whitespace);
        if before_ok && letter_ok && after_ok {
            let mut start = i;
            if start > 0 && text[..start].ends_with(char::is_whitespace) {
                start -= 1;
            }
            let end = i + 5;
            return format!("{}{}", &text[..start], &text[end..]).trim().to_string();
        }
    }
    text.trim().to_string()
}

/// Undo `complete`: restore the priority from `pri:X`, drop the completion date.
pub fn reopen(raw: &str) -> String {
    let t = parse(raw);
    if !t.done {
        return raw.to_string();
    }
    let mut out = String::new();
    if let Some(p) = t.priority {
        out.push_str(&format!("({p}) "));
    }
    if let Some(c) = &t.created {
        out.push_str(c);
        out.push(' ');
    }
    out.push_str(&level_prefix(t.level));
    out.push_str(&remove_pri_tag(&t.text));
    out
}

/// Set (or clear) the priority of an open task.
pub fn set_priority(raw: &str, priority: Option<char>) -> String {
    let t = parse(raw);
    if t.done {
        return raw.to_string();
    }
    let (head, level, body) = split_parts(raw);
    let head = if priority_prefix(head).is_some() { &head[4..] } else { head };
    let pre = level_prefix(level);
    match priority {
        Some(p) => format!("({p}) {head}{pre}{body}"),
        None => format!("{head}{pre}{body}"),
    }
}

/// Prepend a creation date (after the priority) unless the task has one or is done.
pub fn with_creation_date(raw: &str, today: &str) -> String {
    let t = parse(raw);
    if t.done || t.created.is_some() {
        return raw.to_string();
    }
    let pre = level_prefix(t.level);
    match t.priority {
        Some(p) => format!("({p}) {today} {pre}{}", t.text),
        None => format!("{today} {pre}{}", t.text),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueStatus {
    Overdue,
    Today,
    /// Due within the next `SOON_DAYS` days.
    Soon,
    Upcoming,
}

/// Days before the due date from which a task counts as `Soon`.
pub const SOON_DAYS: i64 = 3;

/// A lower-case priority letter at the very start, `(a) text`, becomes `(A) text`.
pub fn fix_priority_case(text: &str) -> String {
    let (pre, rest) = dash_prefix(text).map_or(("".to_string(), text), |(n, r)| (level_prefix(n), r));
    let b = rest.as_bytes();
    if b.len() >= 3 && b[0] == b'(' && b[1].is_ascii_lowercase() && b[2] == b')' {
        format!("{pre}({}){}", (b[1] as char).to_ascii_uppercase(), &rest[3..])
    } else {
        text.to_string()
    }
}

pub fn due_status(t: &Task, today: &str) -> Option<DueStatus> {
    let due = t.tags.get("due")?;
    if !is_date_shape(due) {
        return None;
    }
    Some(match due.as_str().cmp(today) {
        std::cmp::Ordering::Less => DueStatus::Overdue,
        std::cmp::Ordering::Equal => DueStatus::Today,
        std::cmp::Ordering::Greater => {
            let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
            match (d(due), d(today)) {
                (Some(a), Some(b)) if (a - b).num_days() <= SOON_DAYS => DueStatus::Soon,
                _ => DueStatus::Upcoming,
            }
        }
    })
}

// ---------- date expressions (for due:) ----------

static DMY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,2})\.(\d{1,2})\.(\d{4})$").unwrap());
static DM_BARE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,2})\.(\d{1,2})\.$").unwrap());
static DM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d{1,2})\.(\d{1,2})\.?$").unwrap());
static NEXT_WEEK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^next\s*week$").unwrap());
static PLUS_UNIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\+(\d+)\s*(d|days?|w|weeks?|m|mo|months?)$").unwrap());
static NEXT_WEEK_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^next\s+week").unwrap());
static PLUS_UNIT_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\+\d+\s*(?:days?|d|weeks?|w|months?|mo|m)").unwrap());

/// Resolve a typed date expression to a date:
///   2026-12-01, 01.12.2026, 1.12. / 1.12 (next occurrence), today, tomorrow
///   (heute, morgen, übermorgen), next week (+7 days), +3d, +3 days, +2w, +1 month.
/// With `bare` only literal dates (ISO, dd.mm.yyyy, dd.mm.) are accepted.
pub fn parse_date_expr(expr: &str, today: NaiveDate, bare: bool) -> Option<NaiveDate> {
    let s = expr.trim().to_lowercase();
    if is_date_shape(&s) {
        return NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok();
    }
    if let Some(m) = DMY.captures(&s) {
        return NaiveDate::from_ymd_opt(m[3].parse().ok()?, m[2].parse().ok()?, m[1].parse().ok()?);
    }
    let dm = if bare { DM_BARE.captures(&s) } else { DM.captures(&s) };
    if let Some(m) = dm {
        let (d, mo): (u32, u32) = (m[1].parse().ok()?, m[2].parse().ok()?);
        use chrono::Datelike;
        for k in 0..5 {
            if let Some(date) = NaiveDate::from_ymd_opt(today.year() + k, mo, d) {
                if date >= today {
                    return Some(date);
                }
            }
        }
        return None;
    }
    if bare {
        return None;
    }
    let plus = |n: i64| today.checked_add_signed(chrono::Duration::days(n));
    match s.as_str() {
        "today" | "heute" => return plus(0),
        "tomorrow" | "morgen" => return plus(1),
        "übermorgen" => return plus(2),
        _ => {}
    }
    if NEXT_WEEK.is_match(&s) {
        return plus(7);
    }
    if let Some(m) = PLUS_UNIT.captures(&s) {
        let n: u32 = m[1].parse().ok()?;
        return match m[2].chars().next()? {
            'd' => plus(n as i64),
            'w' => plus(7 * n as i64),
            _ => today.checked_add_months(Months::new(n)),
        };
    }
    None
}

/// Length of the date expression starting at `rest` (what follows `due:`).
fn expr_len(rest: &str) -> usize {
    for re in [&*NEXT_WEEK_PREFIX, &*PLUS_UNIT_PREFIX] {
        if let Some(m) = re.find(rest) {
            if rest[m.end()..].chars().next().map_or(true, char::is_whitespace) {
                return m.end();
            }
        }
    }
    rest.find(char::is_whitespace).unwrap_or(rest.len())
}

/// Replace every `due:<expression>` that can be resolved with `due:YYYY-MM-DD`.
pub fn normalize_due(text: &str, today: NaiveDate) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    let mut i = 0;
    while i < text.len() {
        let Some(off) = text[i..].find("due:") else {
            out.push_str(&text[i..]);
            break;
        };
        let p = i + off;
        let at_word_start = p == 0 || text[..p].ends_with(char::is_whitespace);
        if !at_word_start {
            out.push_str(&text[i..p + 4]);
            i = p + 4;
            continue;
        }
        let v = p + 4;
        let len = expr_len(&text[v..]);
        let expr = &text[v..v + len];
        out.push_str(&text[i..v]);
        match parse_date_expr(expr, today, false) {
            Some(d) => out.push_str(&iso(d)),
            None => out.push_str(expr),
        }
        i = v + len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn spec_examples() {
        assert_eq!(parse("(A) Call Mom").priority, Some('A'));
        for l in ["Really gotta call Mom (A) @phone", "(b) Get back", "(B)->Submit"] {
            assert_eq!(parse(l).priority, None, "{l}");
        }
        let t = parse("(A) 2011-03-02 Call Mom +Family @phone");
        assert_eq!(t.created.as_deref(), Some("2011-03-02"));
        assert_eq!(t.text, "Call Mom +Family @phone");
        assert_eq!(t.projects, ["Family"]);
        assert_eq!(t.contexts, ["phone"]);
        assert!(parse("Email a@b.com").contexts.is_empty());
        assert!(parse("Learn 2+2").projects.is_empty());
        let t = parse("x 2011-03-02 2011-03-01 Review +p @c");
        assert!(t.done);
        assert_eq!(t.completed.as_deref(), Some("2011-03-02"));
        assert_eq!(t.created.as_deref(), Some("2011-03-01"));
        for l in ["xylophone lesson", "X 2012-01-01 Make", "(A) x Find"] {
            assert!(!parse(l).done, "{l}");
        }
        assert_eq!(parse("a due:2024-01-02 http://x.y").tags.len(), 1);
    }

    #[test]
    fn complete_reopen_roundtrip() {
        let raw = "(B) 2024-01-01 Write code +x @home";
        let done = complete(raw, "2024-02-02");
        assert_eq!(done, "x 2024-02-02 2024-01-01 Write code +x @home pri:B");
        assert_eq!(parse(&done).priority, Some('B'));
        assert_eq!(reopen(&done), raw);
    }

    #[test]
    fn priority_and_created() {
        assert_eq!(set_priority("Call Mom", Some('A')), "(A) Call Mom");
        assert_eq!(set_priority("(A) Call Mom", Some('C')), "(C) Call Mom");
        assert_eq!(set_priority("(A) Call Mom", None), "Call Mom");
        assert_eq!(with_creation_date("(A) Call", "2024-05-05"), "(A) 2024-05-05 Call");
        assert_eq!(with_creation_date("2024-01-01 Call", "2024-05-05"), "2024-01-01 Call");
    }

    #[test]
    fn date_expressions() {
        let today = d(2026, 11, 10);
        let p = |e: &str| parse_date_expr(e, today, false);
        assert_eq!(p("2026-12-01"), Some(d(2026, 12, 1)));
        assert_eq!(p("01.12.2026"), Some(d(2026, 12, 1)));
        assert_eq!(p("15.11."), Some(d(2026, 11, 15)));
        assert_eq!(p("15.03."), Some(d(2027, 3, 15)));
        assert_eq!(p("15.03"), Some(d(2027, 3, 15)));
        assert_eq!(p("31.02.2026"), None);
        assert_eq!(p("today"), Some(today));
        assert_eq!(p("tomorrow"), Some(d(2026, 11, 11)));
        assert_eq!(p("next week"), Some(d(2026, 11, 17)));
        assert_eq!(p("+3days"), Some(d(2026, 11, 13)));
        assert_eq!(p("+2w"), Some(d(2026, 11, 24)));
        assert_eq!(p("+1month"), Some(d(2026, 12, 10)));
        assert_eq!(p("+3"), None);
        assert_eq!(parse_date_expr("1.5", today, true), None);
        assert_eq!(parse_date_expr("tomorrow", today, true), None);
    }

    #[test]
    fn normalize() {
        let today = d(2026, 11, 10);
        assert_eq!(normalize_due("Buy milk due:tomorrow @x", today), "Buy milk due:2026-11-11 @x");
        assert_eq!(normalize_due("A due:next week B", today), "A due:2026-11-17 B");
        assert_eq!(normalize_due("A due:+3 days", today), "A due:2026-11-13");
        assert_eq!(normalize_due("A due:24.12.", today), "A due:2026-12-24");
        assert_eq!(normalize_due("A due:whenever", today), "A due:whenever");
        assert_eq!(normalize_due("call tomorrow +3d", today), "call tomorrow +3d");
        assert_eq!(normalize_due("overdue:today", today), "overdue:today");
    }

    #[test]
    fn sub_entries() {
        let t = parse("(A) 2026-01-02 -- call +x");
        assert_eq!((t.level, t.priority, t.created.as_deref(), t.text.as_str()), (2, Some('A'), Some("2026-01-02"), "call +x"));
        assert_eq!(parse("-- (A) 2026-01-02 call +x").level, 2); // old form is still read
        assert_eq!(parse("x 2026-02-03 2026-01-02 - job").level, 1);
        assert_eq!(parse("----- five").level, 0);
        assert_eq!(parse("-nospace").level, 0);
        assert_eq!(complete("2026-01-02 - job", "2026-02-03"), "x 2026-02-03 2026-01-02 - job");
        assert_eq!(reopen("x 2026-02-03 2026-01-02 - job pri:B"), "(B) 2026-01-02 - job");
        assert_eq!(set_priority("-- job", Some('C')), "(C) -- job");
        assert_eq!(set_priority("(C) 2026-01-01 -- job", None), "2026-01-01 -- job");
        assert_eq!(with_creation_date("(C) -- job", "2026-05-05"), "(C) 2026-05-05 -- job");
        assert_eq!(fix_priority_case("(a) x"), "(A) x");
        assert_eq!(with_level("(A) 2026-01-01 -- x", 1), "(A) 2026-01-01 - x");
        assert_eq!(with_level("x", 9), "---- x");
        let l: Vec<String> = ["a", "- b", "-- c", "d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(subtree_end(&l, 0), 2);
        assert_eq!(subtree_end(&l, 1), 2);
        assert_eq!(group_heads(&l), vec![0, 0, 0, 3]);
    }
}

//! Splits a line into coloured pieces: priority, done marker, dates, +tags, @contexts, due, key:value.
//! Same rules as `highlight()` in web/composer.js.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tok {
    Plain,
    Priority(char), // '\0' while a priority is still being typed
    Done,
    Date,
    Project,
    Context,
    Due,
    Meta,
    Code, // `inline code`
}

fn is_iso_date(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() == 10 && b.iter().enumerate().all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// `1.12.` / `01.12.2026`
fn is_dotted_date(p: &str) -> bool {
    let parts: Vec<&str> = p.split('.').collect();
    let digits = |s: &str, lo: usize, hi: usize| (lo..=hi).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        [d, m, ""] => digits(d, 1, 2) && digits(m, 1, 2),
        [d, m, y] => digits(d, 1, 2) && digits(m, 1, 2) && digits(y, 4, 4),
        _ => false,
    }
}

/// `(`, `(A`, `(a)` ... while the user is still typing a priority.
fn partial_priority(p: &str) -> Option<char> {
    let inner = p.strip_prefix('(')?;
    let inner = inner.strip_suffix(')').unwrap_or(inner);
    let mut cs = inner.chars();
    match (cs.next(), cs.next()) {
        (None, _) => Some('\0'),
        (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
        _ => None,
    }
}

/// `prefix`: apply the rules that only make sense at the start of a task line (priority, `x`).
pub fn highlight(text: &str, prefix: bool) -> Vec<(String, Tok)> {
    // `code` spans (may contain spaces) are cut out first; the rest is tokenised by words
    let mut out = Vec::new();
    let mut rest = text;
    let mut first = true;
    // sub-entry prefix `- ` .. `---- `
    if prefix {
        let n = text.bytes().take_while(|&b| b == b'-').count();
        if (1..=4).contains(&n) && text.as_bytes().get(n) == Some(&b' ') {
            out.push((text[..n].to_string(), Tok::Date));
            out.push((" ".to_string(), Tok::Plain));
            rest = &text[n + 1..];
        }
    }
    while !rest.is_empty() {
        let span = rest.find('`').and_then(|a| {
            let close = rest[a + 1..].find('`')?;
            (close > 0 && !rest[a + 1..a + 1 + close].contains('\n')).then_some((a, a + close + 2))
        });
        match span {
            Some((a, b)) => {
                if a > 0 {
                    out.extend(words(&rest[..a], prefix && first));
                }
                out.push((rest[a..b].to_string(), Tok::Code));
                rest = &rest[b..];
            }
            None => {
                out.extend(words(rest, prefix && first));
                break;
            }
        }
        first = false;
    }
    out
}

fn words(text: &str, prefix: bool) -> Vec<(String, Tok)> {
    // split into alternating runs of whitespace / non-whitespace, keeping everything
    let mut runs: Vec<(String, bool)> = Vec::new();
    for ch in text.chars() {
        let ws = ch.is_whitespace();
        match runs.last_mut() {
            Some((s, w)) if *w == ws => s.push(ch),
            _ => runs.push((ch.to_string(), ws)),
        }
    }
    let first_is_word = runs.first().is_some_and(|(_, ws)| !ws);
    let mut out = Vec::with_capacity(runs.len());
    for (idx, (p, ws)) in runs.iter().enumerate() {
        let tok = if *ws {
            Tok::Plain
        } else {
            let next_is_space = runs.get(idx + 1).is_some();
            let at_start = idx == 0 && first_is_word;
            let b = p.as_bytes();
            if prefix && at_start && b.len() == 3 && b[0] == b'(' && b[1].is_ascii_uppercase() && b[2] == b')' && next_is_space {
                Tok::Priority(b[1] as char)
            } else if prefix && at_start && partial_priority(p).is_some() {
                Tok::Priority(partial_priority(p).unwrap_or('\0'))
            } else if prefix && at_start && p == "x" && next_is_space {
                Tok::Done
            } else if is_iso_date(p) || is_dotted_date(p) {
                Tok::Date
            } else if p.chars().count() > 1 && p.starts_with('+') {
                Tok::Project
            } else if p.chars().count() > 1 && p.starts_with('@') {
                Tok::Context
            } else if p == "due" || p.starts_with("due:") {
                Tok::Due
            } else if let Some((k, v)) = p.split_once(':') {
                if !k.is_empty() && !v.is_empty() && !v.contains(':') && !v.starts_with("//") { Tok::Meta } else { Tok::Plain }
            } else {
                Tok::Plain
            }
        };
        out.push((p.clone(), tok));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<Tok> {
        highlight(s, true).into_iter().filter(|(t, _)| !t.trim().is_empty()).map(|(_, k)| k).collect()
    }

    #[test]
    fn tokens() {
        assert_eq!(
            kinds("(A) 2026-01-01 Call +Fam @home due:2026-12-01 k:v"),
            [Tok::Priority('A'), Tok::Date, Tok::Plain, Tok::Project, Tok::Context, Tok::Due, Tok::Meta]
        );
        assert_eq!(kinds("x 2026-01-02 done")[0], Tok::Done);
        assert_eq!(kinds("(")[0], Tok::Priority('\0'));
        assert_eq!(kinds("Call 15.03.")[1], Tok::Date);
        assert_eq!(kinds("see http://x.y")[1], Tok::Plain);
        assert_eq!(kinds("run `ls -la +x` now"), vec![Tok::Plain, Tok::Code, Tok::Plain]);
        assert_eq!(kinds("open `unclosed +x"), vec![Tok::Plain, Tok::Plain, Tok::Project]);
        assert_eq!(kinds("-- (A) sub")[..2], [Tok::Date, Tok::Priority('A')]);
        assert_eq!(highlight("-- (A) sub", true).into_iter().map(|(t, _)| t).collect::<String>(), "-- (A) sub");
        // concatenating the pieces gives the original text back
        let s = "  (B)  a  +b ";
        assert_eq!(highlight(s, true).into_iter().map(|(t, _)| t).collect::<String>(), s);
    }
}

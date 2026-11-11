// todo.txt format: parsing and line manipulation.
// Pure functions, no DOM or platform APIs, so the same module can be reused by
// the terminal and desktop apps.
// Spec: ../description/README.md

const DATE_RE = /^\d{4}-\d{2}-\d{2}$/;
const PRIORITY_RE = /^\(([A-Z])\) /;
const PRI_TAG_RE = /(?:^|\s)pri:[A-Z](?=\s|$)/;
const TAG_RE = /^([^\s:]+):([^\s:]+)$/;

/** Format a Date as YYYY-MM-DD in local time. */
export function isoDate(d = new Date()) {
  const p = (n) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

/**
 * Parse one line into its parts.
 * `text` is the description with priority, `x` and dates removed.
 */
export function parse(raw) {
  const task = {
    raw,
    done: false,
    priority: null,
    completed: null,
    created: null,
    text: '',
    projects: [],
    contexts: [],
    tags: {},
  };

  let rest = raw;
  if (rest.startsWith('x ')) {
    task.done = true;
    rest = rest.slice(2);
    let m = /^(\S+) /.exec(rest);
    if (m && DATE_RE.test(m[1])) {
      task.completed = m[1];
      rest = rest.slice(m[0].length);
      m = /^(\S+) /.exec(rest);
      if (m && DATE_RE.test(m[1])) {
        task.created = m[1];
        rest = rest.slice(m[0].length);
      }
    }
  } else {
    const m = PRIORITY_RE.exec(rest);
    if (m) {
      task.priority = m[1];
      rest = rest.slice(m[0].length);
    }
    const d = /^(\d{4}-\d{2}-\d{2}) /.exec(rest);
    if (d) {
      task.created = d[1];
      rest = rest.slice(d[0].length);
    }
  }
  task.text = rest;

  for (const word of rest.split(/\s+/)) {
    if (word.length > 1 && word[0] === '+') task.projects.push(word.slice(1));
    else if (word.length > 1 && word[0] === '@') task.contexts.push(word.slice(1));
    else {
      const m = TAG_RE.exec(word);
      if (m && !m[2].startsWith('//')) task.tags[m[1]] = m[2];
    }
  }
  // Completed tasks keep their priority in a pri:X tag.
  if (task.done && task.tags.pri && /^[A-Z]$/.test(task.tags.pri)) {
    task.priority = task.tags.pri;
  }
  return task;
}

/** Mark a task complete: `x DONE CREATED text [pri:X]`. */
export function complete(raw, today = isoDate()) {
  const t = parse(raw);
  if (t.done) return raw;
  let out = `x ${today} `;
  if (t.created) out += `${t.created} `;
  out += t.text;
  if (t.priority && !t.tags.pri) out += ` pri:${t.priority}`;
  return out;
}

/** Undo `complete`: restore priority from pri:X and drop the completion date. */
export function reopen(raw) {
  const t = parse(raw);
  if (!t.done) return raw;
  const text = t.text.replace(PRI_TAG_RE, '').trim();
  let out = '';
  if (t.priority) out += `(${t.priority}) `;
  if (t.created) out += `${t.created} `;
  return out + text;
}

/** Set (letter A-Z) or clear (null) the priority of an open task. */
export function setPriority(raw, priority) {
  const t = parse(raw);
  if (t.done) return raw;
  const body = raw.replace(PRIORITY_RE, '');
  return priority ? `(${priority}) ${body}` : body;
}

/** Prepend a creation date after the priority, unless one is present. */
export function withCreationDate(raw, today = isoDate()) {
  const t = parse(raw);
  if (t.done || t.created) return raw;
  return (t.priority ? `(${t.priority}) ` : '') + `${today} ${t.text}`;
}

/** Days before the due date from which a task counts as 'soon' due. */
export const SOON_DAYS = 3;

/** 'overdue' | 'today' | 'soon' (within SOON_DAYS) | 'upcoming' | null for the task's due: date. */
export function dueStatus(task, today = isoDate()) {
  const due = task.tags.due;
  if (!due || !DATE_RE.test(due)) return null;
  if (due < today) return 'overdue';
  if (due === today) return 'today';
  const days = Math.round((Date.parse(due + 'T00:00:00Z') - Date.parse(today + 'T00:00:00Z')) / 86400000);
  return days <= SOON_DAYS ? 'soon' : 'upcoming';
}

/** A lower-case priority letter at the very start, `(a) text`, becomes `(A) text`. */
export function fixPriorityCase(text) {
  return text.replace(/^\(([a-z])\)/, (_, c) => `(${c.toUpperCase()})`);
}

/** All note names of a task (`note:NAME` words, without extension), in order: a task can have several notes. */
export function noteNames(task) {
  const out = [];
  for (const w of task.text.split(/\s+/)) {
    const m = /^note:([\w.-]+)$/.exec(w);
    if (m && !m[1].startsWith('.')) out.push(m[1]);
  }
  return out;
}

/** All pasted-image file names of a task (`image:FILE.png` words). */
export function imageNames(task) {
  const out = [];
  for (const w of task.text.split(/\s+/)) {
    const m = /^image:([\w.-]+\.(?:png|jpe?g|gif|webp|bmp))$/i.exec(w);
    if (m && !m[1].startsWith('.')) out.push(m[1]);
  }
  return out;
}

/** Name of the first note file of a task, or null. */
export function noteName(task) {
  return noteNames(task)[0] || null;
}

/** `Note-1`, `Note-2`, ... : the first name not in `used` (compared case-insensitively). */
export function nextNoteName(used) {
  const low = new Set([...used].map((n) => n.toLowerCase()));
  for (let i = 1; ; i++) {
    const cand = `Note-${i}`;
    if (!low.has(cand.toLowerCase())) return cand;
  }
}

/** File-name-safe short name derived from a task description. */
export function slugify(text) {
  const s = text.normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase()
    .replace(/[+@]\S+|\S+:\S+/g, ' ').replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 32).replace(/-+$/, '');
  return s || 'note';
}

/** Split file contents into task lines (blank lines dropped). */
export function parseFile(text) {
  return text.split(/\r?\n/).filter((l) => l.trim() !== '');
}

export function serializeFile(lines) {
  return lines.length ? lines.join('\n') + '\n' : '';
}


// ---------- date expressions (for due:) ----------

function validDate(y, m, d) {
  const dt = new Date(y, m - 1, d);
  return dt.getFullYear() === y && dt.getMonth() === m - 1 && dt.getDate() === d ? dt : null;
}

/**
 * Resolve a typed date expression to a local Date (or null):
 *   2026-12-01, 01.12.2026, 1.12. / 1.12 (next occurrence, this or next year),
 *   today, tomorrow (heute, morgen, übermorgen), next week (+7 days),
 *   +3d, +3days, +2w, +2weeks, +1m, +1month.
 * With `bare: true` only unambiguous literal dates are accepted (ISO, dd.mm.yyyy, dd.mm.),
 * for dates that are not behind `due:` (so "1.5" or "+3d" are left alone).
 */
export function parseDateExpr(expr, today = new Date(), { bare = false } = {}) {
  const s = String(expr).trim().toLowerCase();
  const t0 = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  let m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (m) return validDate(+m[1], +m[2], +m[3]);
  m = /^(\d{1,2})\.(\d{1,2})\.(\d{4})$/.exec(s);
  if (m) return validDate(+m[3], +m[2], +m[1]);
  m = (bare ? /^(\d{1,2})\.(\d{1,2})\.$/ : /^(\d{1,2})\.(\d{1,2})\.?$/).exec(s);
  if (m) {
    for (let k = 0; k < 5; k++) {          // next occurrence; skips Feb 29 in non-leap years
      const d = validDate(t0.getFullYear() + k, +m[2], +m[1]);
      if (d && d >= t0) return d;
    }
    return null;
  }
  if (bare) return null;

  const plus = (n) => new Date(t0.getFullYear(), t0.getMonth(), t0.getDate() + n);
  if (s === 'today' || s === 'heute') return plus(0);
  if (s === 'tomorrow' || s === 'morgen') return plus(1);
  if (s === 'übermorgen') return plus(2);
  if (/^next\s*week$/.test(s)) return plus(7);
  m = /^\+(\d+)\s*(d|days?|w|weeks?|m|mo|months?)$/.exec(s);
  if (m) {
    const n = +m[1];
    const unit = m[2][0] === 'w' ? 'w' : m[2][0] === 'd' ? 'd' : 'm';
    if (unit === 'd') return plus(n);
    if (unit === 'w') return plus(7 * n);
    return new Date(t0.getFullYear(), t0.getMonth() + n, t0.getDate());
  }
  return null;
}

/** Replace every `due:<expression>` that can be resolved with `due:YYYY-MM-DD`. */
export function normalizeDue(text, today = new Date()) {
  const re = /(^|\s)due:(next\s+week|\+\d+\s*(?:days?|d|weeks?|w|months?|mo|m)|\S+)(?=\s|$)/g;
  return text.replace(re, (all, pre, expr) => {
    const d = parseDateExpr(expr, today);
    return d ? `${pre}due:${isoDate(d)}` : all;
  });
}

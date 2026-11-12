import test from 'node:test';
import assert from 'node:assert/strict';
import * as T from '../todotxt.js';

test('priority rules from the spec', () => {
  assert.equal(T.parse('(A) Call Mom').priority, 'A');
  for (const l of ['Really gotta call Mom (A) @phone', '(b) Get back to the boss', '(B)->Submit TPS report']) {
    assert.equal(T.parse(l).priority, null, l);
  }
});

test('creation date rules', () => {
  assert.equal(T.parse('2011-03-02 Document +TodoTxt task format').created, '2011-03-02');
  const t = T.parse('(A) 2011-03-02 Call Mom');
  assert.equal(t.created, '2011-03-02');
  assert.equal(t.text, 'Call Mom');
  assert.equal(T.parse('(A) Call Mom 2011-03-02').created, null);
});

test('projects and contexts', () => {
  const t = T.parse('(A) Call Mom +Family +PeaceLoveAndHappiness @iphone @phone');
  assert.deepEqual(t.projects, ['Family', 'PeaceLoveAndHappiness']);
  assert.deepEqual(t.contexts, ['iphone', 'phone']);
  assert.deepEqual(T.parse('Email SoAndSo at soandso@example.com').contexts, []);
  assert.deepEqual(T.parse('Learn how to add 2+2').projects, []);
});

test('completion rules', () => {
  const t = T.parse("x 2011-03-02 2011-03-01 Review Tim's pull request +TodoTxtTouch @github");
  assert.equal(t.done, true);
  assert.equal(t.completed, '2011-03-02');
  assert.equal(t.created, '2011-03-01');
  for (const l of ['xylophone lesson', 'X 2012-01-01 Make resolutions', '(A) x Find ticket prices']) {
    assert.equal(T.parse(l).done, false, l);
  }
});

test('key:value tags', () => {
  const t = T.parse('Pay rent due:2024-01-02 see http://example.com');
  assert.deepEqual(t.tags, { due: '2024-01-02' });
});

test('complete / reopen round trip keeps priority', () => {
  const raw = '(B) 2024-01-01 Write code +x @home';
  const done = T.complete(raw, '2024-02-02');
  assert.equal(done, 'x 2024-02-02 2024-01-01 Write code +x @home pri:B');
  assert.equal(T.parse(done).priority, 'B');
  assert.equal(T.reopen(done), raw);
});

test('setPriority and withCreationDate', () => {
  assert.equal(T.setPriority('Call Mom', 'A'), '(A) Call Mom');
  assert.equal(T.setPriority('(A) Call Mom', 'C'), '(C) Call Mom');
  assert.equal(T.setPriority('(A) Call Mom', null), 'Call Mom');
  assert.equal(T.withCreationDate('(A) Call Mom', '2024-05-05'), '(A) 2024-05-05 Call Mom');
  assert.equal(T.withCreationDate('2024-01-01 Call Mom', '2024-05-05'), '2024-01-01 Call Mom');
});

test('dueStatus', () => {
  const s = (l) => T.dueStatus(T.parse(l), '2024-06-15');
  assert.equal(s('a due:2024-06-14'), 'overdue');
  assert.equal(s('a due:2024-06-15'), 'today');
  assert.equal(s('a due:2024-06-16'), 'soon');
  assert.equal(s('a due:2024-06-18'), 'soon');
  assert.equal(s('a due:2024-06-19'), 'upcoming');
  assert.equal(s('a'), null);
});

test('file round trip', () => {
  assert.deepEqual(T.parseFile('a\r\n\r\nb\n'), ['a', 'b']);
  assert.equal(T.serializeFile(['a', 'b']), 'a\nb\n');
  assert.equal(T.serializeFile([]), '');
});

test('date expressions', () => {
  const today = new Date(2026, 10, 10); // 2026-11-10
  const iso = (e, o) => { const d = T.parseDateExpr(e, today, o); return d && T.isoDate(d); };
  assert.equal(iso('2026-12-01'), '2026-12-01');
  assert.equal(iso('01.12.2026'), '2026-12-01');
  assert.equal(iso('1.2.2027'), '2027-02-01');
  assert.equal(iso('15.11.'), '2026-11-15');
  assert.equal(iso('15.03.'), '2027-03-15');      // already passed this year -> next year
  assert.equal(iso('15.03'), '2027-03-15');
  assert.equal(iso('31.02.2026'), null);
  assert.equal(iso('today'), '2026-11-10');
  assert.equal(iso('tomorrow'), '2026-11-11');
  assert.equal(iso('next week'), '2026-11-17');
  assert.equal(iso('+3days'), '2026-11-13');
  assert.equal(iso('+3d'), '2026-11-13');
  assert.equal(iso('+2w'), '2026-11-24');
  assert.equal(iso('+1month'), '2026-12-10');
  assert.equal(iso('+3'), null);
  assert.equal(iso('1.5', { bare: true }), null);  // bare words are left alone
  assert.equal(iso('tomorrow', { bare: true }), null);
  assert.equal(iso('15.03.', { bare: true }), '2027-03-15');
});

test('normalizeDue', () => {
  const today = new Date(2026, 10, 10);
  assert.equal(T.normalizeDue('Buy milk due:tomorrow @x', today), 'Buy milk due:2026-11-11 @x');
  assert.equal(T.normalizeDue('A due:next week B', today), 'A due:2026-11-17 B');
  assert.equal(T.normalizeDue('A due:+3 days', today), 'A due:2026-11-13');
  assert.equal(T.normalizeDue('A due:+3d', today), 'A due:2026-11-13');
  assert.equal(T.normalizeDue('A due:24.12.', today), 'A due:2026-12-24');
  assert.equal(T.normalizeDue('A due:2026-12-01 later', today), 'A due:2026-12-01 later');
  assert.equal(T.normalizeDue('A due:whenever', today), 'A due:whenever');
  assert.equal(T.normalizeDue('call tomorrow +3d', today), 'call tomorrow +3d');
});

test('fixPriorityCase / noteName / slugify', () => {
  assert.equal(T.fixPriorityCase('(a) x'), '(A) x');
  assert.equal(T.fixPriorityCase('foo (a)'), 'foo (a)');
  assert.equal(T.noteName(T.parse('a note:my-note')), 'my-note');
  assert.equal(T.noteName(T.parse('a note:../x')), null);
  assert.equal(T.slugify('Call Mom +Family @phone due:2024-01-01'), 'call-mom');
});

test('noteNames / nextNoteName', () => {
  assert.deepEqual(T.noteNames(T.parse('a note:Note-1 note:b.c note:../x')), ['Note-1', 'b.c']);
  assert.equal(T.nextNoteName([]), 'Note-1');
  assert.equal(T.nextNoteName(['Note-1', 'note-2']), 'Note-3');
});

test('sub-entries', () => {
  const t = T.parse('-- (A) 2026-01-02 call +x');
  assert.equal(t.level, 2);
  assert.equal(t.priority, 'A');
  assert.equal(t.created, '2026-01-02');
  assert.equal(t.text, 'call +x');
  assert.equal(T.parse('----- five').level, 0);
  assert.equal(T.parse('-nospace').level, 0);
  assert.equal(T.complete('- 2026-01-02 job', '2026-02-03'), '- x 2026-02-03 2026-01-02 job');
  assert.equal(T.reopen('- x 2026-02-03 2026-01-02 job pri:B'), '- (B) 2026-01-02 job');
  assert.equal(T.setPriority('-- job', 'C'), '-- (C) job');
  assert.equal(T.setPriority('-- (C) job', null), '-- job');
  assert.equal(T.withCreationDate('-- (C) job', '2026-05-05'), '-- (C) 2026-05-05 job');
  assert.equal(T.fixPriorityCase('- (a) x'), '- (A) x');
  assert.equal(T.withLevel('-- x', 1), '- x');
  assert.equal(T.withLevel('x', 9), '---- x');
  const L = ['a', '- b', '-- c', 'd'];
  assert.equal(T.subtreeEnd(L, 0), 2);
  assert.equal(T.subtreeEnd(L, 1), 2);
  assert.deepEqual(T.groupHeads(L), [0, 0, 0, 3]);
});

test('selectForMove takes the sub-tree of every match', () => {
  const L = ['a +p', '- a1', 'b', '- b1 +p', 'c'];
  assert.deepEqual(T.selectForMove(L, ['+p']), [0, 1, 3]);
  assert.deepEqual(T.selectForMove(L, []), []);
  assert.ok(T.matchesTerms('(A) call +Home @x', ['+home', '@X', '(a)', 'CALL']));
});

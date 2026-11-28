// KT-2004 — the diff model DiffViewer reads: parseUnifiedDiff builds it from
// a git patch (old and new line numbers from the hunk headers, renames,
// binary files, added and deleted files), diffFileFromRows from the positional
// pairs of qp-history-diff (a changed line carries both texts). Written before
// the code.
import { describe, expect, it } from 'vitest';
import { diffFileFromRows, newText, parseUnifiedDiff, selectedLines, type DiffFileModel } from './diffModel';
import { diffLines } from './qp-history-diff';

const PATCH = [
  'diff --git a/src/app.ts b/src/app.ts',
  'index 1111111..2222222 100644',
  '--- a/src/app.ts',
  '+++ b/src/app.ts',
  '@@ -10,4 +10,5 @@ export function main() {',
  ' const a = 1;',
  '-const b = 2;',
  '+const b = 3;',
  '+const c = 4;',
  ' return a;',
  ' }',
  '@@ -40,2 +41,2 @@',
  '-old tail',
  '+new tail',
  ' last',
  '\\ No newline at end of file',
  'diff --git a/docs/old-name.md b/docs/new-name.md',
  'similarity index 90%',
  'rename from docs/old-name.md',
  'rename to docs/new-name.md',
  'index 3333333..4444444 100644',
  '--- a/docs/old-name.md',
  '+++ b/docs/new-name.md',
  '@@ -1 +1 @@',
  '-# Old',
  '+# New',
  'diff --git a/assets/logo.png b/assets/logo.png',
  'index 5555555..6666666 100644',
  'Binary files a/assets/logo.png and b/assets/logo.png differ',
  'diff --git a/added.txt b/added.txt',
  'new file mode 100644',
  'index 0000000..7777777',
  '--- /dev/null',
  '+++ b/added.txt',
  '@@ -0,0 +1,2 @@',
  '+one',
  '+two',
  'diff --git a/gone.txt b/gone.txt',
  'deleted file mode 100644',
  'index 8888888..0000000',
  '--- a/gone.txt',
  '+++ /dev/null',
  '@@ -1 +0,0 @@',
  '-bye',
  '',
].join('\n');

describe('parseUnifiedDiff', () => {
  const files = parseUnifiedDiff(PATCH);
  const file = (path: string) => files.find(f => f.path === path)!;

  it('finds one file per diff header, in order', () => {
    expect(files.map(f => f.path)).toEqual(['src/app.ts', 'docs/new-name.md', 'assets/logo.png', 'added.txt', 'gone.txt']);
  });

  it('numbers the old and new lines from the hunk headers', () => {
    const [first, second] = file('src/app.ts').hunks;
    expect(first.header).toBe('@@ -10,4 +10,5 @@ export function main() {');
    expect(first.lines.map(l => [l.kind, l.oldNumber, l.newNumber, l.text])).toEqual([
      ['context', 10, 10, 'const a = 1;'],
      ['removed', 11, undefined, 'const b = 2;'],
      ['added', undefined, 11, 'const b = 3;'],
      ['added', undefined, 12, 'const c = 4;'],
      ['context', 12, 13, 'return a;'],
      ['context', 13, 14, '}'],
    ]);
    expect(second.header).toBe('@@ -40,2 +41,2 @@');
    expect(second.lines.map(l => [l.kind, l.oldNumber, l.newNumber])).toEqual([
      ['removed', 40, undefined],
      ['added', undefined, 41],
      ['context', 41, 42],
      ['meta', undefined, undefined],
    ]);
    expect(second.lines[3].text).toBe('\\ No newline at end of file');
  });

  it('counts the added and removed lines of each file', () => {
    expect(file('src/app.ts')).toMatchObject({ status: 'modified', added: 3, removed: 2 });
    expect(file('added.txt')).toMatchObject({ status: 'added', added: 2, removed: 0 });
    expect(file('gone.txt')).toMatchObject({ status: 'deleted', added: 0, removed: 1 });
  });

  it('reads a rename: its previous path and status', () => {
    expect(file('docs/new-name.md')).toMatchObject({ status: 'renamed', previousPath: 'docs/old-name.md', added: 1, removed: 1 });
    expect(file('docs/new-name.md').hunks[0].lines.map(l => [l.oldNumber, l.newNumber])).toEqual([[1, undefined], [undefined, 1]]);
  });

  it('marks a binary file, with no lines', () => {
    expect(file('assets/logo.png')).toMatchObject({ binary: true, hunks: [] });
  });

  it('keeps the mode line of a new or deleted file as a meta line', () => {
    const [meta] = file('added.txt').hunks;
    expect(meta.header).toBeUndefined();
    expect(meta.lines).toEqual([{ kind: 'meta', text: 'new file mode 100644' }]);
    expect(file('added.txt').hunks[1].lines.map(l => l.newNumber)).toEqual([1, 2]);
  });

  it('reads a patch without a diff --git line, and a blank context line', () => {
    const [plain] = parseUnifiedDiff(['--- a/x.txt', '+++ b/x.txt', '@@ -1,3 +1,3 @@', ' a', '', '-b', '+B'].join('\n'));
    expect(plain.path).toBe('x.txt');
    expect(plain.hunks[0].lines.map(l => [l.kind, l.oldNumber, l.newNumber, l.text])).toEqual([
      ['context', 1, 1, 'a'],
      ['context', 2, 2, ''],
      ['removed', 3, undefined, 'b'],
      ['added', undefined, 3, 'B'],
    ]);
  });

  it('returns no file for an empty text, and keeps unicode', () => {
    expect(parseUnifiedDiff('')).toEqual([]);
    const [u] = parseUnifiedDiff(['--- a/é.md', '+++ b/é.md', '@@ -1 +1 @@', '-café', '+café ☕'].join('\n'));
    expect(u.path).toBe('é.md');
    expect(u.hunks[0].lines[1].text).toBe('café ☕');
  });
});

describe('diffFileFromRows (qp-history-diff positional pairs)', () => {
  it('turns same / changed / added / removed rows into context / changed / added / removed lines', () => {
    const rows = diffLines('alpha\nbeta\ngamma', 'alpha\nBETA\ngamma\ndelta');
    const model = diffFileFromRows(rows, { path: 'prompt.md' });
    expect(model).toMatchObject({ path: 'prompt.md', status: 'modified', added: 2, removed: 1 });
    expect(model.hunks).toHaveLength(1);
    expect(model.hunks[0].lines).toEqual([
      { kind: 'context', oldNumber: 1, newNumber: 1, text: 'alpha' },
      { kind: 'changed', oldNumber: 2, newNumber: 2, text: 'BETA', otherText: 'beta' },
      { kind: 'context', oldNumber: 3, newNumber: 3, text: 'gamma' },
      { kind: 'added', newNumber: 4, text: 'delta' },
    ]);
  });

  it('keeps a removed row on its old line number', () => {
    const model = diffFileFromRows([{ prev: 'x', next: '', kind: 'removed' }], { path: 'a', previousPath: 'b' });
    expect(model.previousPath).toBe('b');
    expect(model.hunks[0].lines).toEqual([{ kind: 'removed', oldNumber: 1, text: 'x' }]);
  });
});

describe('newText and selectedLines', () => {
  const model: DiffFileModel = {
    path: 'f',
    status: 'modified',
    hunks: [
      { header: '@@ -1,2 +1,2 @@', lines: [
        { kind: 'context', oldNumber: 1, newNumber: 1, text: 'keep' },
        { kind: 'removed', oldNumber: 2, text: 'old' },
        { kind: 'added', newNumber: 2, text: 'new' },
      ] },
      { lines: [
        { kind: 'meta', text: '\\ No newline at end of file' },
        { kind: 'changed', oldNumber: 3, newNumber: 3, text: 'after', otherText: 'before' },
      ] },
    ],
  };

  it('writes the new text of a file: context, added and the new side of a changed line', () => {
    expect(newText(model)).toBe('keep\nnew\nafter');
  });

  it('returns the lines of a selection, by their index across the hunks', () => {
    expect(selectedLines(model, { path: 'f', start: 1, end: 2 }).map(l => l.text)).toEqual(['old', 'new']);
    expect(selectedLines(model, { path: 'other', start: 0, end: 1 })).toEqual([]);
  });
});

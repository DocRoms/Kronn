import { describe, expect, it } from 'vitest';
import { splitPatchByFile } from '../commit-patch';

describe('splitPatchByFile (KT-87)', () => {
  it('splits a two-file patch and counts its lines', () => {
    const patch = [
      'diff --git a/src/a.ts b/src/a.ts',
      'index 111..222 100644',
      '--- a/src/a.ts',
      '+++ b/src/a.ts',
      '@@ -1,2 +1,2 @@',
      '-const a = 1;',
      '+const a = 2;',
      ' export default a;',
      'diff --git a/README.md b/README.md',
      'index 333..444 100644',
      '--- a/README.md',
      '+++ b/README.md',
      '@@ -1 +1,2 @@',
      ' # Title',
      '+A new line',
    ].join('\n');

    const files = splitPatchByFile(patch);
    expect(files.map(f => f.path)).toEqual(['src/a.ts', 'README.md']);
    expect(files[0]).toMatchObject({ added: 1, removed: 1, binary: false });
    expect(files[1]).toMatchObject({ added: 1, removed: 0 });
    // `---`/`+++` headers must not be counted as content lines.
    expect(files[0].body).toContain('@@ -1,2 +1,2 @@');
    expect(files[1].body.startsWith('diff --git a/README.md')).toBe(true);
  });

  it('names a rename with both sides', () => {
    const files = splitPatchByFile([
      'diff --git a/old/name.rs b/new/name.rs',
      'similarity index 98%',
      'rename from old/name.rs',
      'rename to new/name.rs',
    ].join('\n'));
    expect(files).toHaveLength(1);
    expect(files[0].path).toBe('old/name.rs → new/name.rs');
  });

  it('flags a binary file instead of pretending it has hunks', () => {
    const files = splitPatchByFile([
      'diff --git a/logo.png b/logo.png',
      'index 0000000..1111111',
      'Binary files /dev/null and b/logo.png differ',
    ].join('\n'));
    expect(files[0]).toMatchObject({ path: 'logo.png', binary: true, added: 0 });
  });

  it('keeps a truncated tail attached to its file rather than dropping it', () => {
    // The 400 KB cap cuts on a line boundary, so the last file can end mid-hunk.
    const files = splitPatchByFile([
      'diff --git a/big.txt b/big.txt',
      '@@ -0,0 +1,40000 @@',
      '+ligne 1',
      '+ligne 2',
    ].join('\n'));
    expect(files).toHaveLength(1);
    expect(files[0].added).toBe(2);
    expect(files[0].body).toContain('+ligne 2');
  });

  it('keeps a preamble visible instead of swallowing it', () => {
    const files = splitPatchByFile([
      'commit noise a merge can print',
      'diff --git a/a.ts b/a.ts',
      '+x',
    ].join('\n'));
    expect(files.map(f => f.path)).toEqual(['…', 'a.ts']);
  });

  it('returns nothing for an empty or blank patch', () => {
    expect(splitPatchByFile('')).toEqual([]);
    expect(splitPatchByFile('   \n  ')).toEqual([]);
  });

  it('handles a quoted path with spaces', () => {
    const files = splitPatchByFile('diff --git "a/my dir/f.ts" "b/my dir/f.ts"');
    expect(files[0].path).toBe('my dir/f.ts');
  });
});

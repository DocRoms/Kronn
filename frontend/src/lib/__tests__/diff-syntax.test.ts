import { describe, it, expect } from 'vitest';
import {
  languageForPath,
  highlightLine,
  parseDiffLines,
} from '../diff-syntax';

describe('languageForPath', () => {
  it('maps common extensions to registered languages', () => {
    expect(languageForPath('src/lib/api.ts')).toBe('typescript');
    expect(languageForPath('App.tsx')).toBe('typescript');
    expect(languageForPath('main.rs')).toBe('rust');
    expect(languageForPath('handler.py')).toBe('python');
    expect(languageForPath('server.go')).toBe('go');
    expect(languageForPath('Pet.java')).toBe('java');
    expect(languageForPath('config.json')).toBe('json');
    expect(languageForPath('docker-compose.yml')).toBe('yaml');
    expect(languageForPath('Cargo.toml')).toBe('ini');
    expect(languageForPath('README.md')).toBe('markdown');
    expect(languageForPath('styles.css')).toBe('css');
    expect(languageForPath('index.html')).toBe('xml');
    expect(languageForPath('deploy.sh')).toBe('shell');
    expect(languageForPath('schema.sql')).toBe('sql');
  });

  it('returns null for unknown or opt-out extensions', () => {
    expect(languageForPath('LICENSE')).toBeNull();
    expect(languageForPath('data.bin')).toBeNull();
    expect(languageForPath('notes.txt')).toBeNull();
    expect(languageForPath('')).toBeNull();
  });

  it('falls back to name-based rules for conventional files', () => {
    expect(languageForPath('Dockerfile')).toBe('bash');
    expect(languageForPath('Makefile')).toBe('bash');
    expect(languageForPath('.env.local')).toBe('ini');
  });

  it('handles leading directories and case-insensitive extensions', () => {
    expect(languageForPath('a/b/c.TS')).toBe('typescript');
    expect(languageForPath('deep/nested/path/main.RS')).toBe('rust');
  });

  it('strips query strings / fragments defensively', () => {
    expect(languageForPath('raw/src/a.ts?v=2')).toBe('typescript');
    expect(languageForPath('raw/src/a.ts#L42')).toBe('typescript');
  });

  it('gitignore-style files are explicitly not highlighted', () => {
    // Highlighting a dotfile listing adds noise; render verbatim.
    expect(languageForPath('.gitignore')).toBeNull();
    expect(languageForPath('.dockerignore')).toBeNull();
  });
});

describe('highlightLine', () => {
  it('wraps identifiers/keywords in hljs spans for a known language', () => {
    const html = highlightLine('let foo = 42;', 'typescript');
    // Token classes are hljs-specific; we just assert SOME span was emitted.
    expect(html).toMatch(/<span class="hljs-/);
    // The original text survives, modulo escaping.
    expect(html).toContain('foo');
    expect(html).toContain('42');
  });

  it('returns HTML-escaped text when no language is provided', () => {
    const html = highlightLine('<div class="a">', null);
    expect(html).toBe('&lt;div class=&quot;a&quot;&gt;');
  });

  it('returns empty string for empty input', () => {
    expect(highlightLine('', 'typescript')).toBe('');
  });

  it('does not throw on syntactically incomplete lines', () => {
    // A single `}` on its own would normally be illegal in a fresh
    // parser state — `ignoreIllegals` guards against that.
    expect(() => highlightLine('}', 'typescript')).not.toThrow();
    expect(() => highlightLine('pub fn foo() -> Result<', 'rust')).not.toThrow();
  });

  it('falls back to escaped text for unregistered languages', () => {
    // `cobol` is not registered — must not crash, must escape output.
    const html = highlightLine('<x>', 'cobol');
    expect(html).toBe('&lt;x&gt;');
  });
});

describe('parseDiffLines', () => {
  const sample = [
    'diff --git a/src/main.rs b/src/main.rs',
    'index abc..def 100644',
    '--- a/src/main.rs',
    '+++ b/src/main.rs',
    '@@ -1,3 +1,4 @@',
    ' fn main() {',
    '-    let x = 1;',
    '+    let x = 2;',
    '+    let y = 3;',
    ' }',
  ].join('\n');

  it('classifies every line into add / del / hunk / meta / context', () => {
    const parsed = parseDiffLines(sample);
    expect(parsed.map(p => p.kind)).toEqual([
      'meta',     // diff --git
      'meta',     // index
      'meta',     // ---
      'meta',     // +++
      'hunk',     // @@
      'context',  // ` fn main() {`
      'del',      // -    let x = 1;
      'add',      // +    let x = 2;
      'add',      // +    let y = 3;
      'context',  // ` }`
    ]);
  });

  it('strips the leading `+` / `-` / ` ` from content', () => {
    const parsed = parseDiffLines('+added\n-removed\n ctx');
    expect(parsed.map(p => p.content)).toEqual(['added', 'removed', 'ctx']);
  });

  it('preserves the raw line for copy-paste / debugging', () => {
    const [hunk] = parseDiffLines('@@ -1,2 +1,3 @@');
    expect(hunk.raw).toBe('@@ -1,2 +1,3 @@');
    expect(hunk.content).toBe('@@ -1,2 +1,3 @@');
  });

  it('recognizes renames, deletions, and binary markers as meta', () => {
    const parsed = parseDiffLines(
      'rename from old.rs\nrename to new.rs\ndeleted file mode 100644\nBinary files a/b and c/d differ',
    );
    for (const p of parsed) expect(p.kind).toBe('meta');
  });

  it('handles empty input gracefully', () => {
    expect(parseDiffLines('')).toEqual([{ kind: 'context', content: '', raw: '' }]);
  });
});

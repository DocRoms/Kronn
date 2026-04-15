/**
 * Syntax highlighting for the git diff view in `GitPanel`.
 *
 * Design choices:
 *  - Load `highlight.js` via `lib/core` and register a curated set of
 *    languages by hand (~10 languages bundled explicitly). Avoids the
 *    190-language default (~900 KB) — the curated list still covers 95%+
 *    of the files shown in a typical dev session.
 *  - Highlight **one line at a time**, not the whole diff. Multi-line
 *    tokens (block comments, template literals) won't always match
 *    perfectly but the tradeoff is worth it: we sidestep the "reassemble
 *    highlighted HTML back into per-line segments" problem entirely, and
 *    the `-` (deletion) lines are rendered as flat red anyway — tokens
 *    don't carry across them.
 *  - `ignoreIllegals: true` so a line that isn't syntactically complete
 *    on its own (e.g. a dangling closing brace) still renders instead of
 *    throwing.
 *  - Output HTML is safe: the input is whatever git produced (trusted
 *    local repository) and hljs escapes it internally. We return it as
 *    `string` so the caller can inject via `dangerouslySetInnerHTML`.
 */

import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import css from 'highlight.js/lib/languages/css';
import go from 'highlight.js/lib/languages/go';
import ini from 'highlight.js/lib/languages/ini'; // also handles TOML-ish
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import markdown from 'highlight.js/lib/languages/markdown';
import python from 'highlight.js/lib/languages/python';
import rust from 'highlight.js/lib/languages/rust';
import shell from 'highlight.js/lib/languages/shell';
import sql from 'highlight.js/lib/languages/sql';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml'; // HTML lives under `xml` in hljs
import yaml from 'highlight.js/lib/languages/yaml';

// Register once at module load. `registerLanguage` is idempotent, but a
// module-level side-effect keeps the call site for each language visible
// in one place — useful when debugging missing highlights.
hljs.registerLanguage('bash', bash);
hljs.registerLanguage('css', css);
hljs.registerLanguage('go', go);
hljs.registerLanguage('ini', ini);
hljs.registerLanguage('java', java);
hljs.registerLanguage('javascript', javascript);
hljs.registerLanguage('json', json);
hljs.registerLanguage('markdown', markdown);
hljs.registerLanguage('python', python);
hljs.registerLanguage('rust', rust);
hljs.registerLanguage('shell', shell);
hljs.registerLanguage('sql', sql);
hljs.registerLanguage('typescript', typescript);
hljs.registerLanguage('xml', xml);
hljs.registerLanguage('yaml', yaml);

/**
 * Map a filename / path to a highlight.js language id.
 * Returns `null` when we don't want to highlight at all — the caller
 * should then render the text verbatim. This is on purpose for files
 * where highlighting adds noise (binaries, minified bundles, plain txt).
 */
export function languageForPath(path: string): string | null {
  if (!path) return null;
  // Strip query/fragment if the caller passed a URL-ish path.
  const clean = path.split(/[?#]/)[0];
  const base = clean.toLowerCase();
  // Extension match first — cheap and deterministic.
  const extMatch = base.match(/\.([a-z0-9]+)$/);
  const ext = extMatch ? extMatch[1] : '';

  // Name-based overrides for files without a conventional extension.
  const name = base.split('/').pop() ?? base;
  if (name === 'dockerfile' || name.endsWith('.dockerfile')) return 'bash'; // close enough for a diff view
  if (name === 'makefile') return 'bash';
  if (name === '.env' || name.startsWith('.env.')) return 'ini';
  if (name === '.gitignore' || name === '.dockerignore') return null;

  switch (ext) {
    case 'ts': case 'tsx':
    case 'js': case 'jsx': case 'mjs': case 'cjs':
      // `typescript` parses JS cleanly and handles JSX; no separate `tsx` id.
      return 'typescript';
    case 'rs':
      return 'rust';
    case 'py':
      return 'python';
    case 'go':
      return 'go';
    case 'java':
      return 'java';
    case 'json': case 'jsonc':
      return 'json';
    case 'yaml': case 'yml':
      return 'yaml';
    case 'toml':
    case 'ini': case 'conf': case 'cfg':
      return 'ini';
    case 'md': case 'mdx':
      return 'markdown';
    case 'css': case 'scss': case 'sass': case 'less':
      return 'css';
    case 'html': case 'htm': case 'xml': case 'svg':
      return 'xml';
    case 'sh': case 'bash': case 'zsh': case 'fish':
      return 'shell';
    case 'sql':
      return 'sql';
    default:
      return null;
  }
}

/**
 * Highlight a single line of source in the given language.
 * Returns HTML-safe output ready for `dangerouslySetInnerHTML`.
 *
 * Empty lines return an empty string — the caller should render `&nbsp;`
 * or a min-height so diff alignment is preserved visually.
 */
export function highlightLine(line: string, language: string | null): string {
  if (!line) return '';
  if (!language) return escapeHtml(line);
  try {
    const out = hljs.highlight(line, { language, ignoreIllegals: true });
    return out.value;
  } catch {
    // hljs can throw for unregistered languages or internal errors; fall
    // back to a safe escaped string so the diff still renders legibly.
    return escapeHtml(line);
  }
}

/** Minimal HTML-escape for the no-language fallback. */
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

/** Discriminant for a single line in a unified diff. */
export type DiffLineKind = 'add' | 'del' | 'hunk' | 'meta' | 'context';

export interface DiffLine {
  kind: DiffLineKind;
  /** Raw content WITHOUT the leading prefix char (`+`, `-`, ` `). */
  content: string;
  /** Original full line (prefix included) — kept for copy-paste / debug. */
  raw: string;
}

/**
 * Classify every line of a unified-diff string.
 *
 * `+++`, `---`, `diff --git`, `index `, `new file` etc. are tagged `meta`
 * so the renderer can style them as header chrome (dim grey) instead of
 * confusing them with actual add/delete content.
 */
export function parseDiffLines(diff: string): DiffLine[] {
  const rawLines = diff.split('\n');
  return rawLines.map<DiffLine>(raw => {
    if (raw.startsWith('+++') || raw.startsWith('---')) {
      return { kind: 'meta', content: raw, raw };
    }
    if (raw.startsWith('@@')) {
      return { kind: 'hunk', content: raw, raw };
    }
    if (
      raw.startsWith('diff ') ||
      raw.startsWith('index ') ||
      raw.startsWith('new file') ||
      raw.startsWith('deleted file') ||
      raw.startsWith('rename ') ||
      raw.startsWith('similarity ') ||
      raw.startsWith('Binary files ')
    ) {
      return { kind: 'meta', content: raw, raw };
    }
    if (raw.startsWith('+')) {
      return { kind: 'add', content: raw.slice(1), raw };
    }
    if (raw.startsWith('-')) {
      return { kind: 'del', content: raw.slice(1), raw };
    }
    // Context line — may start with a leading space (standard unified
    // diff) or be empty (stripped terminator between hunks).
    const content = raw.startsWith(' ') ? raw.slice(1) : raw;
    return { kind: 'context', content, raw };
  });
}

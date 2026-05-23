// Heal Mermaid `sequenceDiagram` sources whose participant/actor alias is a
// reserved keyword.
//
// Mermaid's lexer matches block keywords (`alt`, `loop`, `end`, …)
// case-insensitively, so an alias like `participant Alt as …` makes every
// `X->>Alt:` read as the start of an `alt` block → "Expecting ... got 'alt'"
// parse error. This is a recurring trap in AI-generated diagrams (the audit
// loves `Alt` for AlternateLocaleSubscriber, `End` for an endpoint, …).
//
// Two-layer defense: the audit prompt now forbids these aliases (prevention),
// and this sanitizer renames any that slip through BEFORE the diagram is
// parsed (cure) — so existing diagrams render without a re-audit.
//
// Safety: only the STRUCTURAL part of each line is rewritten. Message text
// (everything after the first `:`) and the `as <label>` of a declaration are
// left byte-for-byte intact, so we never corrupt prose that merely contains
// the word.

// Reserved sequenceDiagram keywords (lower-cased) that collide when used as a
// participant/actor identifier.
const SEQUENCE_RESERVED = new Set([
  'alt', 'else', 'end', 'opt', 'loop', 'par', 'and', 'rect', 'note',
  'critical', 'option', 'break', 'activate', 'deactivate', 'participant',
  'actor', 'autonumber', 'box', 'create', 'destroy', 'link', 'links',
]);

// `participant X`, `participant X as Label`, `actor X`, `actor X as Label`.
// Captures: (1) prefix incl. keyword, (2) alias token, (3) optional ` as …`.
const DECL_RE = /^(\s*(?:participant|actor)\s+)([^\s:]+)((?:\s+as\b.*)?)$/;

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Returns `source` unchanged unless it is a `sequenceDiagram` that declares a
 * participant/actor alias colliding with a reserved keyword — in which case
 * each colliding alias (and every structural reference) is renamed to a safe
 * `<alias>_` variant.
 */
export function sanitizeMermaidSource(source: string): string {
  const firstToken = source.trimStart().split(/[\s\n]/, 1)[0] ?? '';
  if (firstToken !== 'sequenceDiagram') return source;

  const lines = source.split('\n');

  // 1. Collect declared aliases.
  const declared = new Set<string>();
  for (const line of lines) {
    const m = DECL_RE.exec(line);
    if (m) declared.add(m[2]);
  }

  // 2. Which collide with a reserved keyword (case-insensitive)?
  const colliding = [...declared].filter(a => SEQUENCE_RESERVED.has(a.toLowerCase()));
  if (colliding.length === 0) return source;

  // 3. Pick a safe replacement for each: append `_` until it's neither a
  //    reserved word nor an already-used alias.
  const rename = new Map<string, string>();
  const taken = new Set(declared);
  for (const alias of colliding) {
    let safe = `${alias}_`;
    while (SEQUENCE_RESERVED.has(safe.toLowerCase()) || taken.has(safe)) safe += '_';
    rename.set(alias, safe);
    taken.add(safe);
  }

  const renameTokens = (text: string): string => {
    let out = text;
    for (const [alias, safe] of rename) {
      out = out.replace(new RegExp(`\\b${escapeRegExp(alias)}\\b`, 'g'), safe);
    }
    return out;
  };

  // 4. Rewrite line by line.
  return lines.map(line => {
    const decl = DECL_RE.exec(line);
    if (decl) {
      // Declaration: rename only the alias token, keep `as <label>` verbatim.
      const newAlias = rename.get(decl[2]) ?? decl[2];
      return decl[1] + newAlias + decl[3];
    }
    // Other lines: structural part is everything before the first `:`; the
    // message text after it is never touched.
    const colon = line.indexOf(':');
    if (colon === -1) return renameTokens(line);
    return renameTokens(line.slice(0, colon)) + line.slice(colon);
  }).join('\n');
}

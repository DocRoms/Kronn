// KT-1750 — files an operating system or a file manager drops in any folder
// (`.DS_Store`, `Thumbs.db`, …) never count as a worker's changes: they block
// neither a delivery nor the cleanup of a checkout, and are excluded from
// `git status` in every lane Kronn creates. The list lives in Settings > Workers.

/** Out-of-the-box list, mirrored from `DEFAULT_OS_JUNK_PATTERNS`. */
export const DEFAULT_OS_JUNK_PATTERNS = ['.DS_Store', 'Thumbs.db', 'desktop.ini', '._*'];
/** Most patterns, and longest one, the backend accepts. */
export const MAX_OS_JUNK_PATTERNS = 50;
export const MAX_OS_JUNK_PATTERN_LEN = 100;

/** The list a comma-separated field holds: trimmed, empty entries dropped. */
export function parseOsJunkPatterns(text: string): string[] {
  return text.split(',').map(pattern => pattern.trim()).filter(pattern => pattern !== '');
}

/** The field text for a list. */
export function formatOsJunkPatterns(patterns: readonly string[]): string {
  return patterns.join(', ');
}

/** Why `pattern` cannot be a pattern (the backend refuses the same), or null. */
export function osJunkPatternProblem(pattern: string): string | null {
  if (pattern === '' || pattern.length > MAX_OS_JUNK_PATTERN_LEN) return pattern;
  // A pattern is written into the repository's local exclude: what git would read
  // as something else (a comment, a negation, a path, a class) is refused.
  if (/\s/.test(pattern) || pattern.startsWith('#') || pattern.startsWith('!')) return pattern;
  if (/[/\\[\]?]/.test(pattern)) return pattern;
  if ((pattern.match(/\*/g) ?? []).length > 1) return pattern;
  if (/^[*.]+$/.test(pattern)) return pattern;
  return null;
}

/** The first refused pattern of a list, or null when it can be saved. */
export function osJunkListProblem(patterns: readonly string[]): string | null {
  if (patterns.length > MAX_OS_JUNK_PATTERNS) return `> ${MAX_OS_JUNK_PATTERNS}`;
  for (const pattern of patterns) {
    const problem = osJunkPatternProblem(pattern);
    if (problem !== null) return problem;
  }
  return null;
}

// Pure helper used by WorkflowsPage's QA batch form. Extracted from the
// page component so the parsing rules — newline/comma/semicolon split for
// 1-variable QAs, JSON array required for multi-variable QAs — can be unit
// tested without spinning up React or i18n. Error messages are i18n KEYS
// (the caller resolves them with `t()`); this keeps the helper pure and
// the keys untouched in test snapshots.

export type ParseResult = { items: unknown[] } | { errorKey: string; errorArg?: string };

export interface QuickApiVarShape {
  /** Number of declared variables on the QA — drives single vs multi mode. */
  varCount: number;
}

/**
 * Parse the textarea content into a JSON array of items.
 *
 * Rules:
 *   - QA with 1 variable → each non-empty line/value (split on `\n,;`)
 *     becomes a string in the result array. Trimmed, deduped (preserves
 *     order of first occurrence).
 *   - QA with 2+ variables → user MUST paste a JSON array. Anything else
 *     surfaces a parse-error key the caller renders.
 *   - QA with 0 variables → treated as 1-variable mode (each line is one
 *     standalone call, the executor falls back to `{{batch.item}}`).
 *
 * Returns either `{ items }` or `{ errorKey }` (and optional `errorArg`
 * for the JSON-parse error message).
 */
export function parseBatchQAItems(qa: QuickApiVarShape, raw: string): ParseResult {
  const trimmed = raw.trim();
  if (!trimmed) return { errorKey: 'qa.batch.errorEmpty' };

  // 2+ vars → JSON array of objects required.
  if (qa.varCount > 1) {
    try {
      const parsed = JSON.parse(trimmed);
      if (!Array.isArray(parsed)) return { errorKey: 'qa.batch.errorJsonArray' };
      if (parsed.length === 0) return { errorKey: 'qa.batch.errorEmpty' };
      return { items: parsed };
    } catch (e) {
      return { errorKey: 'qa.batch.errorJsonParse', errorArg: String(e) };
    }
  }

  // 1 var (or 0 vars treated as 1) → split on newlines, commas, semicolons.
  // Trim each, drop empties, dedupe preserving first-seen order.
  const items = [...new Set(
    trimmed.split(/[\n,;]/).map(s => s.trim()).filter(Boolean)
  )];
  if (items.length === 0) return { errorKey: 'qa.batch.errorEmpty' };
  return { items };
}

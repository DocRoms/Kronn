// Auto-derived JSONPath suggestions from a real API response.
//
// Why this exists: the wizard's static `Examples` row (`$.[0]`, `$.total`)
// only fires when the user already understands JSONPath. Most users don't.
// Showing them 3-5 paths derived from THEIR response — labelled in plain
// French and with a sample of what they'd actually get out — turns the
// extract step from "guess the syntax" into "click the right answer".
//
// Algorithm in one sentence: walk the response (depth ≤ 3), surface every
// array (with item count + likely scalar field for "Tous les <field>"),
// every top-level scalar that smells like a counter (`total`, `count`,
// `totalCount`), and the first item path of any array (handy for "tester
// sur le premier élément avant de fan-out").

export interface PathSuggestion {
  /** JSONPath expression to apply when the chip is clicked. */
  path: string;
  /** Translation key consumed by the i18n layer. Falls back to `rawLabel`
   *  when the host doesn't have a matching key. */
  i18nKey: string;
  /** Variables interpolated into the i18n template (`{0}`, `{1}` …). */
  args: (string | number)[];
  /** Truncated preview of what the path resolves to — shown next to the
   *  chip so the user can spot the right one without applying it first. */
  sample: string;
  /** Heuristic kind, used by tests + for ordering. */
  kind: 'array-iter' | 'array-field' | 'array-first' | 'count' | 'scalar';
}

/** Common scalar field names that make an array of objects useful as a
 *  fan-out source. Order matters — first match wins. */
const SCALAR_FIELD_PRIORITY = [
  'id', 'key', 'name', 'title', 'path', 'url', 'slug', 'email', 'pseudo',
];

/** Likely counter fields surfaced by REST APIs. */
const COUNT_FIELD_PRIORITY = ['total', 'count', 'totalCount', 'total_count', 'length'];

const MAX_DEPTH = 3;
const MAX_SUGGESTIONS = 6;

interface Discovered {
  path: string;
  value: unknown;
  depth: number;
}

/** Walk the JSON value and collect (path, value) pairs up to MAX_DEPTH. */
function walk(value: unknown, path: string, depth: number, acc: Discovered[]): void {
  if (depth > MAX_DEPTH) return;
  acc.push({ path, value, depth });
  if (Array.isArray(value)) {
    // Don't recurse into every item — we only need the first one to know
    // the array's element shape.
    if (value.length > 0) walk(value[0], `${path}[0]`, depth + 1, acc);
  } else if (value && typeof value === 'object') {
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      const seg = /^[A-Za-z_][A-Za-z0-9_]*$/.test(k) ? `.${k}` : `['${k.replace(/'/g, "\\'")}']`;
      walk(v, path + seg, depth + 1, acc);
    }
  }
}

/** Truncate a value-as-string for the chip preview. */
function previewOf(value: unknown, max = 28): string {
  let s: string;
  if (value == null) s = 'null';
  else if (typeof value === 'string') s = `"${value}"`;
  else if (typeof value === 'number' || typeof value === 'boolean') s = String(value);
  else if (Array.isArray(value)) s = `[${value.length}]`;
  else s = '{…}';
  return s.length > max ? `${s.slice(0, max)}…` : s;
}

/** Pick the first scalar key in an item whose name matches our priority
 *  list, or the first scalar key at all if none match. Returns null when
 *  the array isn't homogeneously made of objects with at least one scalar. */
function pickScalarField(items: unknown[]): string | null {
  if (items.length === 0) return null;
  const first = items[0];
  if (!first || typeof first !== 'object' || Array.isArray(first)) return null;
  const obj = first as Record<string, unknown>;
  for (const key of SCALAR_FIELD_PRIORITY) {
    const v = obj[key];
    if (typeof v === 'string' || typeof v === 'number') return key;
  }
  for (const [k, v] of Object.entries(obj)) {
    if (typeof v === 'string' || typeof v === 'number') return k;
  }
  return null;
}

/**
 * Derive up to MAX_SUGGESTIONS path suggestions from a response body.
 * The first chip is always the most useful one for the response's shape.
 */
export function suggestPaths(value: unknown): PathSuggestion[] {
  if (value == null || typeof value !== 'object') return [];
  const found: Discovered[] = [];
  walk(value, '$', 0, found);

  const out: PathSuggestion[] = [];

  // ─── Arrays first (the most actionable thing for fan-out workflows) ──
  for (const { path, value: v } of found) {
    if (!Array.isArray(v)) continue;
    const arr = v as unknown[];

    // (a) "Tous les <field>" if items have a scalar of interest.
    const field = pickScalarField(arr);
    if (field) {
      const sampleItem = arr[0] as Record<string, unknown>;
      out.push({
        path: `${path}[*].${field}`,
        i18nKey: 'wf.apicall.suggest.allField',
        args: [field, arr.length],
        sample: previewOf(sampleItem[field]),
        kind: 'array-field',
      });
    }

    // (b) Iterate over the whole array (raw items, useful when next step
    //     is a BatchQuickPrompt that templates over the object itself).
    out.push({
      path: `${path}[*]`,
      i18nKey: 'wf.apicall.suggest.iterate',
      args: [arr.length],
      sample: arr.length > 0 ? previewOf(arr[0]) : '[]',
      kind: 'array-iter',
    });

    // (c) First item — handy for "tester avant fan-out".
    if (arr.length > 0) {
      out.push({
        path: `${path}[0]`,
        i18nKey: 'wf.apicall.suggest.firstItem',
        args: [],
        sample: previewOf(arr[0]),
        kind: 'array-first',
      });
    }
  }

  // ─── Counter fields ───────────────────────────────────────────────────
  for (const { path, value: v } of found) {
    if (typeof v !== 'number') continue;
    const lastSegMatch = path.match(/(?:\.|\['])([A-Za-z_][A-Za-z0-9_]*)(?:'\])?$/);
    const segment = lastSegMatch?.[1] ?? '';
    if (COUNT_FIELD_PRIORITY.includes(segment)) {
      out.push({
        path,
        i18nKey: 'wf.apicall.suggest.counter',
        args: [segment, v],
        sample: String(v),
        kind: 'count',
      });
    }
  }

  // De-dup by path (the same path can be reached by walking + by counter
  // detection if the response is unusual) and clip.
  const seen = new Set<string>();
  const unique: PathSuggestion[] = [];
  for (const s of out) {
    if (seen.has(s.path)) continue;
    seen.add(s.path);
    unique.push(s);
  }
  return unique.slice(0, MAX_SUGGESTIONS);
}

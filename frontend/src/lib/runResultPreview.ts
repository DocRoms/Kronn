/**
 * Card-sized readings of a run's result. A card is a summary: it shows what a
 * human looks for first and folds the raw payload away, never the reverse. A
 * Quick API result can weigh megabytes; printing its first 20 000 characters
 * of JSON told nobody what came back.
 */

const PREVIEW_ROWS = 5;
const PREVIEW_COLUMNS = 4;
const CELL_CHARS = 80;
/** Fields a reader recognises a row by, tried first when picking columns. */
const PREFERRED_COLUMNS = ['key', 'number', 'id', 'name', 'title', 'summary', 'status', 'state', 'url', 'html_url'];

type Scalar = string | number | boolean;

function isScalar(value: unknown): value is Scalar {
  return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean';
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function cell(value: unknown): string {
  if (!isScalar(value)) return '';
  const text = String(value);
  return text.length > CELL_CHARS ? `${text.slice(0, CELL_CHARS)}…` : text;
}

export interface TablePreview {
  /** The field the rows came from when the payload wraps them (`issues`). */
  label: string | null;
  columns: string[];
  rows: string[][];
  total: number;
}

export interface ListPreview {
  label: string | null;
  items: string[];
  total: number;
}

export interface QuickApiPreview {
  status: string | null;
  summary: string | null;
  table: TablePreview | null;
  list: ListPreview | null;
  entries: Array<[string, string]> | null;
}

/** The rows a payload carries: the array itself, or the first non-empty
 *  array it wraps. */
function rowsOf(data: unknown): { label: string | null; items: unknown[] } | null {
  if (Array.isArray(data)) return data.length > 0 ? { label: null, items: data } : null;
  if (!isRecord(data)) return null;
  for (const [key, value] of Object.entries(data)) {
    if (Array.isArray(value) && value.length > 0) return { label: key, items: value };
  }
  return null;
}

function tableOf(label: string | null, items: unknown[]): TablePreview | null {
  const records = items.filter(isRecord);
  if (records.length === 0) return null;
  const scalarKeys = Object.keys(records[0]).filter(key => isScalar(records[0][key]));
  const columns = [
    ...PREFERRED_COLUMNS.filter(key => scalarKeys.includes(key)),
    ...scalarKeys.filter(key => !PREFERRED_COLUMNS.includes(key)),
  ].slice(0, PREVIEW_COLUMNS);
  if (columns.length === 0) return null;
  return {
    label,
    columns,
    rows: records.slice(0, PREVIEW_ROWS).map(record => columns.map(column => cell(record[column]))),
    total: items.length,
  };
}

/** A flat object read as `key → value`, or nothing if any value is nested. */
export function flatEntries(value: unknown): Array<[string, string]> | null {
  if (!isRecord(value)) return null;
  const entries = Object.entries(value);
  if (entries.length === 0 || !entries.every(([, entry]) => isScalar(entry) || entry === null)) return null;
  return entries.map(([key, entry]) => [key, entry === null ? '—' : cell(entry)]);
}

/** A Quick API result: `{status, summary, data}`, as the broker records it. */
export function quickApiPreview(result: unknown): QuickApiPreview | null {
  if (!isRecord(result) || !('data' in result || 'summary' in result)) return null;
  const status = typeof result.status === 'string' ? result.status : null;
  const summary = typeof result.summary === 'string' ? result.summary : null;
  const rows = rowsOf(result.data);
  const table = rows ? tableOf(rows.label, rows.items) : null;
  const list = rows && !table && rows.items.every(isScalar)
    ? { label: rows.label, items: rows.items.slice(0, PREVIEW_ROWS).map(cell), total: rows.items.length }
    : null;
  const entries = !rows ? flatEntries(result.data) : null;
  return { status, summary, table, list, entries };
}

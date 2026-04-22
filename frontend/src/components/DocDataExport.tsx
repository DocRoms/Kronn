import { useState, useMemo } from 'react';
import { FileDown, FileSpreadsheet, Presentation, Loader2, ExternalLink, AlertTriangle } from 'lucide-react';
import { docs as docsApi } from '../lib/api';

/** Structured-data export — XLSX / CSV / PPTX.
 *
 *  Unlike <DocPreview> which wraps an HTML document + iframe, these
 *  formats need JSON-shaped input (rows × cols, or slides). Agents
 *  produce the payload in a ```` ```kronn-doc-data ```` fence with a
 *  `format: "xlsx" | "csv" | "pptx"` discriminator, and this component
 *  renders a compact card with one download button per format — no
 *  preview (structured data renders badly in an iframe, and the
 *  spreadsheet / slide experience is the rendering target anyway).
 *
 *  Payload shapes (all single JSON document):
 *    format: "csv"  → { rows: [[...], [...]], delimiter?: string }
 *    format: "xlsx" → { sheets: [{ name, rows: [[...]] }] }
 *    format: "pptx" → { slides: [{ title?, content?, bullets? }] }
 */

// Cell type mirrors the API contract (`lib/api.ts`). Keeping the narrow
// union here avoids an `unknown[]` cast at the call site — the agent is
// trusted to emit JSON-serialisable scalars.
type Cell = string | number | boolean | null;
type CsvPayload = { rows: Cell[][]; delimiter?: string };
type XlsxPayload = { sheets: Array<{ name: string; rows: Cell[][] }> };
type PptxPayload = { slides: Array<{ title?: string; content?: string; bullets?: string[] }> };
type Format = 'csv' | 'xlsx' | 'pptx';

interface DocDataExportProps {
  /** Raw JSON payload (already parsed from the fence's string content).
   *  Shape depends on `format`. Validation happens at click time so a
   *  malformed payload still shows its error inline rather than
   *  crashing the chat. */
  payload: unknown;
  format: Format;
  discussionId: string;
}

export function DocDataExport({ payload, format, discussionId }: DocDataExportProps) {
  const [state, setState] = useState<
    | { kind: 'idle' }
    | { kind: 'loading' }
    | { kind: 'ready'; url: string; filename: string; size: number }
    | { kind: 'error'; message: string }
  >({ kind: 'idle' });

  // Surface a short summary of what's being exported — useful for the
  // user to verify the parsed payload matches what they expected.
  const summary = useMemo(() => summarizePayload(format, payload), [format, payload]);

  const run = async () => {
    setState({ kind: 'loading' });
    try {
      const res =
        format === 'csv'
          ? await docsApi.generateCsv({
              discussion_id: discussionId,
              ...(payload as CsvPayload),
            })
          : format === 'xlsx'
          ? await docsApi.generateXlsx({
              discussion_id: discussionId,
              ...(payload as XlsxPayload),
            })
          : await docsApi.generatePptx({
              discussion_id: discussionId,
              ...(payload as PptxPayload),
            });
      const fallback = `document.${format}`;
      setState({
        kind: 'ready',
        url: res.download_url,
        filename: res.path.split('/').pop() ?? fallback,
        size: res.size_bytes,
      });
    } catch (e) {
      setState({
        kind: 'error',
        message: e instanceof Error ? e.message : String(e),
      });
    }
  };

  const icon = format === 'pptx' ? <Presentation size={12} /> : <FileSpreadsheet size={12} />;
  const label = format.toUpperCase();

  return (
    <div className="doc-data-export">
      <div className="doc-data-export-header">
        {icon}
        <span className="doc-data-export-label">{label}</span>
        <span className="doc-data-export-summary">{summary}</span>
      </div>
      <div className="doc-data-export-actions">
        <button
          type="button"
          className="doc-preview-btn"
          onClick={run}
          disabled={state.kind === 'loading'}
        >
          {state.kind === 'loading' ? (
            <><Loader2 size={11} className="doc-preview-spinner" /> Generating…</>
          ) : (
            <><FileDown size={11} /> Export {label}</>
          )}
        </button>
      </div>
      {state.kind === 'ready' && (
        <a
          className="doc-preview-download"
          href={state.url}
          download={state.filename}
          target="_blank"
          rel="noopener noreferrer"
        >
          <ExternalLink size={11} />
          <span>{state.filename}</span>
          <span className="doc-preview-size">({formatBytes(state.size)})</span>
        </a>
      )}
      {state.kind === 'error' && (
        <div className="doc-preview-error">
          <AlertTriangle size={11} style={{ marginRight: 6 }} />
          {state.message}
        </div>
      )}
    </div>
  );
}

/** Produce a one-liner summary so the user sees at a glance what the
 *  agent prepared: row count, sheet count, slide count. Defensive: a
 *  malformed payload shows a neutral "(invalid payload)" marker instead
 *  of crashing the render. */
function summarizePayload(format: Format, payload: unknown): string {
  try {
    if (format === 'csv') {
      const p = payload as CsvPayload;
      return `${p.rows?.length ?? 0} rows`;
    }
    if (format === 'xlsx') {
      const p = payload as XlsxPayload;
      const sheets = p.sheets ?? [];
      const totalRows = sheets.reduce((n, s) => n + (s.rows?.length ?? 0), 0);
      return `${sheets.length} sheet${sheets.length !== 1 ? 's' : ''}, ${totalRows} rows`;
    }
    if (format === 'pptx') {
      const p = payload as PptxPayload;
      return `${p.slides?.length ?? 0} slides`;
    }
    return '';
  } catch {
    return '(invalid payload)';
  }
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

import { useState, useRef, useEffect } from 'react';
import { FileText, FileDown, Loader2, ExternalLink } from 'lucide-react';
import { docs as docsApi } from '../lib/api';

interface DocPreviewProps {
  /** Full HTML document composed by the agent — used both for the live
   *  preview iframe and as the payload when the user clicks "Generate
   *  PDF". Stored as-is; the agent is responsible for semantic markup. */
  html: string;
  /** Discussion id — the backend uses it to decide the output directory
   *  (`~/.kronn/generated/<discussion_id>/`) so files group per-disc. */
  discussionId: string;
}

/** Live HTML preview + export buttons for an agent-authored document.
 *
 *  Rendered inline inside a chat bubble when the agent wraps a document
 *  in a ```` ```kronn-doc-preview ```` fenced code block. The preview
 *  itself is a sandboxed iframe — no scripts, no form submission — so
 *  hostile HTML can't exfiltrate data or break out of the chat. Export
 *  buttons call the corresponding Kronn docs endpoint and render a
 *  download link once the file is ready.
 *
 *  In phase 1 this component will also listen for an SSE event the
 *  backend emits mid-stream; for now the fence in the markdown is the
 *  single signal. */
export function DocPreview({ html, discussionId }: DocPreviewProps) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  // Per-format generation state. DOCX and PDF share the same HTML
  // input so the iframe preview applies to both; the user picks at
  // export time.
  type GenState =
    | { kind: 'idle' }
    | { kind: 'loading' }
    | { kind: 'ready'; url: string; filename: string; size: number }
    | { kind: 'error'; message: string };
  const [pdfState, setPdfState] = useState<GenState>({ kind: 'idle' });
  const [docxState, setDocxState] = useState<GenState>({ kind: 'idle' });

  // Inject HTML into the sandboxed iframe via srcdoc — same-origin-style
  // rendering without contaminating our document tree. srcdoc is set
  // via attribute + useEffect so React never marshals `html` through
  // its own markup parser (dangerouslySetInnerHTML would defeat the
  // iframe isolation).
  useEffect(() => {
    const el = iframeRef.current;
    if (!el) return;
    el.srcdoc = html;
  }, [html]);

  // Factor out the request + state-transitions so PDF and DOCX share
  // the exact same plumbing. Any future HTML-based format drops in here
  // as a new `call` arm.
  const runExport = async (
    format: 'pdf' | 'docx',
    setState: (s: GenState) => void,
  ) => {
    setState({ kind: 'loading' });
    try {
      const res =
        format === 'pdf'
          ? await docsApi.generatePdf({ discussion_id: discussionId, html, page_size: 'A4' })
          : await docsApi.generateDocx({ discussion_id: discussionId, html });
      const fallback = format === 'pdf' ? 'document.pdf' : 'document.docx';
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

  return (
    <div className="doc-preview">
      <div className="doc-preview-header">
        <FileText size={12} />
        <span>Preview</span>
      </div>
      <iframe
        ref={iframeRef}
        className="doc-preview-iframe"
        // `allow-same-origin` is intentionally absent — without it,
        // scripts can't touch cookies/localStorage and can't navigate
        // top. `allow-forms` is also off so a submit button can't
        // exfiltrate. The preview is viewing-only.
        sandbox=""
        title="Document preview"
      />
      <div className="doc-preview-actions">
        <button
          type="button"
          className="doc-preview-btn"
          onClick={() => runExport('pdf', setPdfState)}
          disabled={pdfState.kind === 'loading'}
        >
          {pdfState.kind === 'loading' ? (
            <><Loader2 size={11} className="doc-preview-spinner" /> Generating…</>
          ) : (
            <><FileDown size={11} /> PDF</>
          )}
        </button>
        <button
          type="button"
          className="doc-preview-btn"
          onClick={() => runExport('docx', setDocxState)}
          disabled={docxState.kind === 'loading'}
        >
          {docxState.kind === 'loading' ? (
            <><Loader2 size={11} className="doc-preview-spinner" /> Generating…</>
          ) : (
            <><FileDown size={11} /> DOCX</>
          )}
        </button>
      </div>
      {/* Result rows — one per format that has generated something.
          Order is stable (PDF first, DOCX second) so the UI reads
          deterministically even when the user exports both. */}
      {pdfState.kind === 'ready' && (
        <a
          className="doc-preview-download"
          href={pdfState.url}
          download={pdfState.filename}
          target="_blank"
          rel="noopener noreferrer"
        >
          <ExternalLink size={11} />
          <span>{pdfState.filename}</span>
          <span className="doc-preview-size">({formatBytes(pdfState.size)})</span>
        </a>
      )}
      {docxState.kind === 'ready' && (
        <a
          className="doc-preview-download"
          href={docxState.url}
          download={docxState.filename}
          target="_blank"
          rel="noopener noreferrer"
        >
          <ExternalLink size={11} />
          <span>{docxState.filename}</span>
          <span className="doc-preview-size">({formatBytes(docxState.size)})</span>
        </a>
      )}
      {pdfState.kind === 'error' && (
        <div className="doc-preview-error">{pdfState.message}</div>
      )}
      {docxState.kind === 'error' && (
        <div className="doc-preview-error">{docxState.message}</div>
      )}
    </div>
  );
}

/** Human-readable file size — "48 KB" / "1.2 MB". */
function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

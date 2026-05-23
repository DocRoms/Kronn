/**
 * 0.8.3 (#289) — Mermaid renderer.
 *
 * Renders a `mermaid` fenced block visually. Used by:
 *   - AiDocViewer (sequence diagrams in docs/architecture/sequences/*.md)
 *   - MessageBubble (agent output that happens to include a mermaid block)
 *
 * Failure modes are explicit: invalid syntax → fall back to the raw
 * source inside a `<pre>` with a small notice. Empty source → render
 * nothing (caller can decide what to show).
 *
 * The `mermaid` library is loaded dynamically (lazy import) so the
 * initial bundle stays light — only mounted when the user actually
 * opens a doc that contains a diagram.
 */
import { useEffect, useRef, useState, useId, memo } from 'react';
import { createPortal } from 'react-dom';
import { ChevronDown, ChevronRight, AlertTriangle, Maximize2, Printer, X } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { sanitizeMermaidSource } from '../lib/mermaidSanitize';
import './MermaidDiagram.css';

interface MermaidDiagramProps {
  /** Raw mermaid source (without the opening ```mermaid / closing ``` fence). */
  source: string;
}

/**
 * Lazy-load mermaid once per session (kept as a module-level promise so
 * subsequent renders don't re-import). The init call sets a neutral
 * theme that respects Kronn's `--kr-bg-*` palette.
 */
let mermaidPromise: Promise<typeof import('mermaid').default> | null = null;
function getMermaid() {
  if (!mermaidPromise) {
    mermaidPromise = import('mermaid').then(mod => {
      const mermaid = mod.default;
      mermaid.initialize({
        startOnLoad: false,
        // 'neutral' is the Mermaid theme that matches our dark backgrounds
        // most cleanly. It also renders correctly on light themes.
        theme: 'neutral',
        // Disable user-supplied click bindings — security guard against
        // a malicious mermaid block injecting JS via `click <id> "javascript:…"`.
        securityLevel: 'strict',
        // Bigger fonts — the default 14px is unreadable on dense graphs.
        themeVariables: { fontSize: '15px' },
      });
      return mermaid;
    });
  }
  return mermaidPromise;
}

// Wrapped in `memo` so an unrelated parent re-render (e.g. Dashboard's
// 3s `auditStatusAll` polling that re-renders the project list while
// the user is in AiDocViewer) does NOT unmount + remount this
// component — which would reset the `fullscreen` useState back to
// false and make the overlay disappear "by itself" every 3 seconds.
// 0.8.3 user bug report.
function MermaidDiagramImpl({ source }: MermaidDiagramProps) {
  const { t } = useT();
  const containerRef = useRef<HTMLDivElement>(null);
  const fullscreenRef = useRef<HTMLDivElement>(null);
  // Stable element id for the mermaid render call (mermaid uses it to
  // namespace internal CSS rules; collisions on the same page would
  // leak styles between diagrams).
  const id = useId().replace(/:/g, '-');
  const [error, setError] = useState<string | null>(null);
  const [showSource, setShowSource] = useState(false);
  const [fullscreen, setFullscreen] = useState(false);
  // Cached SVG markup. Stored so we can re-inject it into the
  // fullscreen overlay's DOM node when the user toggles fullscreen
  // (the inline container's innerHTML is the source of truth).
  const svgMarkupRef = useRef<string>('');

  useEffect(() => {
    let cancelled = false;
    setError(null);
    if (!source.trim()) return;

    // Streaming guard: while an agent is still writing the message,
    // the markdown can arrive with an opening ```mermaid fence and no
    // closing fence yet — the upstream `pre` interceptor still hands
    // us the partial body. Mermaid v11 renders such input as an error
    // SVG ("Syntax error in text · mermaid version 11.15.0") that
    // gets injected via innerHTML. Heuristic: a valid diagram source
    // must start with a known Mermaid root keyword. If it doesn't,
    // skip the render altogether — the surrounding block will hide
    // automatically since `error` stays null and the containerRef
    // is empty until the next valid source.
    const head = source.trimStart().split(/[\s\n]/, 1)[0] ?? '';
    const validRoots = [
      'flowchart', 'graph', 'sequenceDiagram', 'classDiagram',
      'stateDiagram', 'stateDiagram-v2', 'erDiagram', 'journey',
      'gantt', 'pie', 'gitGraph', 'C4Context', 'C4Container',
      'C4Component', 'C4Dynamic', 'C4Deployment', 'requirementDiagram',
      'mindmap', 'timeline', 'sankey-beta', 'xychart-beta',
      'block-beta', 'packet-beta',
    ];
    if (!validRoots.some(root => head === root || head.startsWith(root))) {
      // Not a valid Mermaid root — likely a streaming-in-progress
      // chunk or a mis-tagged code block. Leave the container empty
      // (the source toggle still works if the user wants to see why).
      return;
    }

    void (async () => {
      try {
        const mermaid = await getMermaid();
        if (cancelled) return;
        // `render` returns `{ svg, bindFunctions? }`. We inline the SVG
        // and don't wire bindFunctions because securityLevel:'strict'
        // disables interactive bindings anyway.
        // Heal reserved-keyword participant aliases (e.g. `Alt`) before the
        // parse — see lib/mermaidSanitize. No-op when there's no collision.
        const { svg } = await mermaid.render(`kronn-mmd-${id}`, sanitizeMermaidSource(source));
        if (cancelled) return;
        // Mermaid v11 stopped reliably throwing on parse errors and
        // returns an *error SVG* instead ("Syntax error in text"
        // followed by `mermaid version X.Y.Z`). Detect it and route
        // through our own fallback so the user gets the same UX as
        // a thrown error (notice + raw source) instead of mermaid's
        // ugly inline error glyph.
        if (svg.includes('aria-roledescription="error"') || /Syntax error in (text|graph)/i.test(svg)) {
          setError('Mermaid syntax error (returned by mermaid.render — see source below).');
          return;
        }
        svgMarkupRef.current = svg;
        if (containerRef.current) {
          containerRef.current.innerHTML = svg;
        }
      } catch (e) {
        if (cancelled) return;
        // Mermaid throws helpful messages with parse error location;
        // surface them so the user can fix the source (or report).
        setError(e instanceof Error ? e.message : String(e));
      }
    })();

    return () => { cancelled = true; };
  }, [source, id]);

  // Inject the cached SVG into the fullscreen overlay when it opens.
  // useEffect rather than `dangerouslySetInnerHTML` so we can also
  // close on Escape without re-rendering the underlying SVG (which
  // would force a full mermaid.render() round-trip).
  useEffect(() => {
    if (fullscreen && fullscreenRef.current && svgMarkupRef.current) {
      fullscreenRef.current.innerHTML = svgMarkupRef.current;
    }
  }, [fullscreen]);

  // Escape closes fullscreen.
  useEffect(() => {
    if (!fullscreen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setFullscreen(false);
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [fullscreen]);

  /**
   * Print the diagram by opening a tiny popup window with just the
   * SVG (no chrome, no nav), kicking the browser's print dialog, and
   * closing the popup after the user hits OK/Cancel.
   *
   * Why a popup over `window.print()` on the main doc + `@media print`:
   * the main doc has 100+ unrelated DOM nodes that would all need
   * print-suppression rules. A popup with literally one SVG inside
   * is simpler, more reliable across browsers, and trivially scales
   * the diagram to fit the page (CSS in the popup is fully isolated).
   */
  const handlePrint = () => {
    if (!svgMarkupRef.current) return;
    const win = window.open('', '_blank', 'width=900,height=700');
    if (!win) return; // popup blocker — nothing we can do; user must allow popups
    win.document.write(`<!DOCTYPE html>
<html lang="${document.documentElement.lang || 'fr'}">
<head>
<meta charset="utf-8">
<title>Kronn — Diagramme</title>
<style>
  html, body { margin: 0; padding: 24px; background: white; }
  svg { max-width: 100%; height: auto; display: block; margin: 0 auto; }
  @media print {
    @page { margin: 12mm; }
    body { padding: 0; }
  }
</style>
</head>
<body>${svgMarkupRef.current}<script>window.addEventListener('load',()=>{setTimeout(()=>{window.print();},100);});<\/script></body>
</html>`);
    win.document.close();
  };

  if (!source.trim()) return null;

  return (
    <div className="kronn-mermaid">
      {error ? (
        // Parse error: show a clear notice + the raw source so the
        // user can spot the typo. NEVER show a blank space — the
        // diagram is the whole point of the block.
        <div className="kronn-mermaid-error">
          <p>
            <AlertTriangle size={12} /> {t('mermaid.parseError')}
          </p>
          <details>
            <summary>{t('mermaid.errorDetails')}</summary>
            <pre>{error}</pre>
          </details>
          <pre className="kronn-mermaid-source-fallback">{source}</pre>
        </div>
      ) : (
        <>
          <div ref={containerRef} className="kronn-mermaid-svg" />
          <div className="kronn-mermaid-actions">
            <button
              type="button"
              className="kronn-mermaid-action-btn"
              onClick={() => setFullscreen(true)}
              title={t('mermaid.fullscreen')}
              aria-label={t('mermaid.fullscreen')}
            >
              <Maximize2 size={12} /> {t('mermaid.fullscreen')}
            </button>
            <button
              type="button"
              className="kronn-mermaid-action-btn"
              onClick={handlePrint}
              title={t('mermaid.print')}
              aria-label={t('mermaid.print')}
            >
              <Printer size={12} /> {t('mermaid.print')}
            </button>
            <button
              type="button"
              className="kronn-mermaid-source-toggle"
              onClick={() => setShowSource(s => !s)}
              aria-expanded={showSource}
            >
              {showSource ? <ChevronDown size={10} /> : <ChevronRight size={10} />}
              {t(showSource ? 'mermaid.hideSource' : 'mermaid.showSource')}
            </button>
          </div>
          {showSource && (
            <pre className="kronn-mermaid-source">{source}</pre>
          )}
          {fullscreen && createPortal(
            // 0.8.3 — rendered via Portal on document.body so the
            // overlay survives even if a parent in the markdown
            // subtree happens to remount. The Portal node lives
            // outside the AiDocViewer / MessageBubble subtree;
            // React only unmounts it when THIS component unmounts.
            //
            // Outside-click closes the overlay. The inner content
            // stops propagation so clicking the diagram itself stays
            // open (otherwise dragging a small zoomed-out area would
            // dismiss accidentally).
            <div
              className="kronn-mermaid-fullscreen-overlay"
              onClick={() => setFullscreen(false)}
              role="dialog"
              aria-modal="true"
              aria-label={t('mermaid.fullscreen')}
            >
              <div
                className="kronn-mermaid-fullscreen-content"
                onClick={e => e.stopPropagation()}
              >
                <button
                  type="button"
                  className="kronn-mermaid-fullscreen-close"
                  onClick={() => setFullscreen(false)}
                  aria-label={t('mermaid.closeFullscreen')}
                >
                  <X size={20} />
                </button>
                <div ref={fullscreenRef} className="kronn-mermaid-fullscreen-svg" />
              </div>
            </div>,
            document.body
          )}
        </>
      )}
    </div>
  );
}

// Memo gate: re-render only when `source` actually changes string.
// Unrelated parent state updates (audit polling, project refetch, etc.)
// will hit the cache and leave the inner state intact.
export const MermaidDiagram = memo(MermaidDiagramImpl, (prev, next) => prev.source === next.source);

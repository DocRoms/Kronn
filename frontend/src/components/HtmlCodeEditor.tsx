import { useMemo, useRef, useState } from 'react';
import type { ChangeEvent, KeyboardEvent, UIEvent } from 'react';
import { highlightHtmlLine } from '../lib/html-syntax';
import { buildHtmlPreviewDocument } from '../lib/html-preview';
import { diffLines } from '../lib/qp-history-diff';
import { PAGE_HTML_VERY_HEAVY_BYTES } from '../lib/live-page-weight';

// Highlighting re-tokenizes every line on each keystroke: seconds on an 18 MiB
// Page. Above the red weight grade the code stays plain text (#252). Characters
// stand in for bytes here, a cheap lower bound of the UTF-8 size.
const isTooHeavyToHighlight = (html: string) => html.length > PAGE_HTML_VERY_HEAVY_BYTES;

interface HtmlCodeEditorProps {
  value: string;
  onChange: (value: string) => void;
  ariaLabel: string;
}

export function HtmlCodeEditor({ value, onChange, ariaLabel }: HtmlCodeEditorProps) {
  const highlightRef = useRef<HTMLPreElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const plain = isTooHeavyToHighlight(value);
  const lines = useMemo(() => (plain ? [] : value.split('\n')), [plain, value]);
  const highlightedLines = useMemo(
    () => lines.map(highlightHtmlLine),
    [lines],
  );

  const synchronizeScroll = (event: UIEvent<HTMLTextAreaElement>) => {
    const { scrollLeft, scrollTop } = event.currentTarget;
    if (highlightRef.current) {
      highlightRef.current.scrollLeft = scrollLeft;
      highlightRef.current.scrollTop = scrollTop;
    }
    if (gutterRef.current) gutterRef.current.scrollTop = scrollTop;
  };

  const insertIndent = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key !== 'Tab') return;
    event.preventDefault();
    const textarea = event.currentTarget;
    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const next = `${value.slice(0, start)}  ${value.slice(end)}`;
    onChange(next);
    requestAnimationFrame(() => {
      textarea.selectionStart = start + 2;
      textarea.selectionEnd = start + 2;
    });
  };

  return (
    <div className="live-pages-code-editor" data-plain={plain || undefined}>
      {!plain && (
        <div className="live-pages-code-gutter" ref={gutterRef} aria-hidden="true">
          {lines.map((_, index) => <span key={index}>{index + 1}</span>)}
        </div>
      )}
      <div className="live-pages-code-pane">
        {!plain && (
          <pre ref={highlightRef} className="live-pages-code-highlight" aria-hidden="true">
            {highlightedLines.map((line, index) => (
              <span key={index} dangerouslySetInnerHTML={{ __html: line || '&nbsp;' }} />
            ))}
          </pre>
        )}
        <textarea
          value={value}
          onChange={(event: ChangeEvent<HTMLTextAreaElement>) => onChange(event.target.value)}
          onScroll={synchronizeScroll}
          onKeyDown={insertIndent}
          aria-label={ariaLabel}
          spellCheck={false}
          wrap="off"
        />
      </div>
    </div>
  );
}

interface HtmlRevisionDiffProps {
  previous: string;
  current: string;
  previousLabel: string;
  currentLabel: string;
  codeLabel: string;
  previewLabel: string;
  previewLimitations: string;
}

export function HtmlRevisionDiff({
  previous,
  current,
  previousLabel,
  currentLabel,
  codeLabel,
  previewLabel,
  previewLimitations,
}: HtmlRevisionDiffProps) {
  const rows = useMemo(() => diffLines(previous, current), [current, previous]);
  const plain = isTooHeavyToHighlight(previous) || isTooHeavyToHighlight(current);
  const lineCode = (line: string) => plain
    ? <code>{line || '\u00a0'}</code>
    : <code dangerouslySetInnerHTML={{ __html: highlightHtmlLine(line) || '&nbsp;' }} />;
  const [view, setView] = useState<'code' | 'preview'>('code');
  return (
    <div className="live-pages-revision-comparison" data-testid="live-page-html-comparison">
      <div className="live-pages-revision-view-toggle" role="group" aria-label={codeLabel}>
        <button type="button" aria-pressed={view === 'code'} onClick={() => setView('code')}>{codeLabel}</button>
        <button type="button" aria-pressed={view === 'preview'} onClick={() => setView('preview')}>{previewLabel}</button>
      </div>
      {view === 'code' ? (
        <div className="live-pages-html-diff" data-testid="live-page-html-diff">
          <header><span>{previousLabel}</span><span>{currentLabel}</span></header>
          <div className="live-pages-html-diff-body">
            {rows.map((row, index) => (
              <div key={index} className="live-pages-html-diff-row" data-kind={row.kind}>
                <span className="live-pages-html-diff-line">
                  <i>{index + 1}</i>
                  {lineCode(row.prev)}
                </span>
                <span className="live-pages-html-diff-line">
                  <i>{index + 1}</i>
                  {lineCode(row.next)}
                </span>
              </div>
            ))}
          </div>
        </div>
      ) : (
        <div className="live-pages-html-preview-diff" data-testid="live-page-html-preview-diff">
          <section>
            <header>{previousLabel}</header>
            <iframe sandbox="" srcDoc={buildHtmlPreviewDocument(previous)} title={`${previewLabel}: ${previousLabel}`} />
            <p>{previewLimitations}</p>
          </section>
          <section>
            <header>{currentLabel}</header>
            <iframe sandbox="" srcDoc={buildHtmlPreviewDocument(current)} title={`${previewLabel}: ${currentLabel}`} />
            <p>{previewLimitations}</p>
          </section>
        </div>
      )}
    </div>
  );
}

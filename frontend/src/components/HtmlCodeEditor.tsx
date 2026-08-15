import { useMemo, useRef } from 'react';
import type { ChangeEvent, KeyboardEvent, UIEvent } from 'react';
import { highlightHtmlLine } from '../lib/html-syntax';
import { diffLines } from '../lib/qp-history-diff';

interface HtmlCodeEditorProps {
  value: string;
  onChange: (value: string) => void;
  ariaLabel: string;
}

export function HtmlCodeEditor({ value, onChange, ariaLabel }: HtmlCodeEditorProps) {
  const highlightRef = useRef<HTMLPreElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const lines = useMemo(() => value.split('\n'), [value]);
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
    <div className="live-pages-code-editor">
      <div className="live-pages-code-gutter" ref={gutterRef} aria-hidden="true">
        {lines.map((_, index) => <span key={index}>{index + 1}</span>)}
      </div>
      <div className="live-pages-code-pane">
        <pre ref={highlightRef} className="live-pages-code-highlight" aria-hidden="true">
          {highlightedLines.map((line, index) => (
            <span key={index} dangerouslySetInnerHTML={{ __html: line || '&nbsp;' }} />
          ))}
        </pre>
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
}

export function HtmlRevisionDiff({
  previous,
  current,
  previousLabel,
  currentLabel,
}: HtmlRevisionDiffProps) {
  const rows = useMemo(() => diffLines(previous, current), [current, previous]);
  return (
    <div className="live-pages-html-diff" data-testid="live-page-html-diff">
      <header><span>{previousLabel}</span><span>{currentLabel}</span></header>
      <div className="live-pages-html-diff-body">
        {rows.map((row, index) => (
          <div key={index} className="live-pages-html-diff-row" data-kind={row.kind}>
            <span className="live-pages-html-diff-line">
              <i>{index + 1}</i>
              <code dangerouslySetInnerHTML={{ __html: highlightHtmlLine(row.prev) || '&nbsp;' }} />
            </span>
            <span className="live-pages-html-diff-line">
              <i>{index + 1}</i>
              <code dangerouslySetInnerHTML={{ __html: highlightHtmlLine(row.next) || '&nbsp;' }} />
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}

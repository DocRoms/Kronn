import {
  Fragment, useId, useLayoutEffect, useMemo, useRef, useState,
  type MouseEvent, type ReactNode, type RefObject,
} from 'react';
import { WrapText } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import {
  fileLines, newText,
  type DiffFileModel, type DiffFileStatus, type DiffLineModel, type DiffSelection,
} from '../../lib/diffModel';
import { Button } from './Button';
import { CopyButton } from './CopyButton';
import { IconButton } from './IconButton';
import { LoadingState } from './LoadingState';
import { StatusBadge } from './StatusBadge';
import type { StatusTone } from './statusTones';
import {
  DIFF_HIGHLIGHT_LIMIT, DIFF_MARKERS, DIFF_MAX_SELECTION, DIFF_SPLIT_MIN_REM,
  type DiffLayout, type DiffMaxHeight,
} from './diffKinds';
import './visuallyHidden.css';
import './DiffViewer.css';

export interface DiffViewerProps {
  /** The files to show, as a model (lib/diffModel.ts: parseUnifiedDiff for a
   *  git patch, diffFileFromRows for positional pairs). */
  files: readonly DiffFileModel[];
  /** What the diff is; names the scroll region when `maxHeight` bounds it.
   *  Default "Changes". */
  label?: string;
  /** The layout to start with (default unified). Side by side shows as
   *  unified while the viewer's own width is under 40rem. */
  layout?: DiffLayout;
  /** Told when the reader picks the other layout. */
  onLayoutChange?: (layout: DiffLayout) => void;
  /** Start wrapped (default) or scrolling sideways; the toolbar toggles it. */
  wrap?: boolean;
  /** Bounds the files' height: the scroll area becomes a focusable region. */
  maxHeight?: DiffMaxHeight;
  /** Returns the highlighted markup of a line (escaped HTML: it is injected
   *  as is), e.g. `(text, path) => highlightLine(text, languageForPath(path))`
   *  from lib/diff-syntax.ts. Applied to the new texts: context, added and
   *  the new side of a changed line. */
  highlight?: (text: string, path: string) => string;
  /** A file with more lines than this is not highlighted. Default 2000. */
  highlightLimit?: number;
  /** A CopyButton per file: copies the new text of its shown lines. Default on. */
  copyable?: boolean;
  /** Each line gets a toggle button: a click selects it, shift-click extends
   *  the range from the line clicked first. */
  selectable?: boolean;
  /** Controlled: the selected range, or null. */
  selection?: DiffSelection | null;
  onSelectionChange?: (selection: DiffSelection | null) => void;
  /** The most lines a range holds. Default 50. */
  maxSelection?: number;
  /** The caller's controls for the selection (comment, copy), shown under
   *  its last line. */
  selectionToolbar?: ReactNode;
  loading?: boolean;
  /** The loading sentence; default "Loading…". */
  loadingLabel?: string;
  /** When there is no file. Default "No change". */
  empty?: ReactNode;
  className?: string;
}

const STATUS_TONES: Record<DiffFileStatus, StatusTone> = {
  added: 'success',
  deleted: 'danger',
  modified: 'info',
  renamed: 'neutral',
  copied: 'neutral',
};

/** Whether `el` is narrower than side by side needs. A width of 0 (not laid
 *  out yet, or jsdom) is not narrow. */
function useNarrow(ref: RefObject<HTMLElement | null>): boolean {
  const [narrow, setNarrow] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      const rem = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      const width = el.clientWidth;
      setNarrow(width > 0 && width < DIFF_SPLIT_MIN_REM * rem);
    };
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, [ref]);
  return narrow;
}

/** One line of a file and its index across the file's hunks. */
interface Indexed { line: DiffLineModel; index: number }

/** A row of the side-by-side layout: a meta line across, or an old and a new
 *  side (either may be empty). A context or changed line is one index on
 *  both sides. */
type SplitRow =
  | { meta: Indexed }
  | { left?: Indexed; right?: Indexed; shared: boolean };

/** Pairs each run of removed lines with the run of added lines after it. */
function splitRows(lines: readonly Indexed[]): SplitRow[] {
  const rows: SplitRow[] = [];
  let i = 0;
  while (i < lines.length) {
    const item = lines[i];
    const { kind } = item.line;
    if (kind === 'meta') { rows.push({ meta: item }); i++; continue; }
    if (kind === 'context' || kind === 'changed') { rows.push({ left: item, right: item, shared: true }); i++; continue; }
    const removed: Indexed[] = [];
    while (i < lines.length && lines[i].line.kind === 'removed') removed.push(lines[i++]);
    const added: Indexed[] = [];
    while (i < lines.length && lines[i].line.kind === 'added') added.push(lines[i++]);
    for (let k = 0; k < Math.max(removed.length, added.length); k++) {
      rows.push({ left: removed[k], right: added[k], shared: false });
    }
  }
  return rows;
}

interface FileProps {
  file: DiffFileModel;
  layout: DiffLayout;
  highlight?: (text: string, path: string) => string;
  highlightLimit: number;
  copyable: boolean;
  selectable: boolean;
  selection: DiffSelection | null;
  onToggle: (path: string, index: number, extend: boolean) => void;
  selectionToolbar?: ReactNode;
}

function DiffFile({ file, layout, highlight, highlightLimit, copyable, selectable, selection, onToggle, selectionToolbar }: FileProps) {
  const { t } = useT();
  const pathId = useId();
  const lines = useMemo(() => fileLines(file), [file]);
  const showLines = !file.binary && !file.truncated;
  const canHighlight = Boolean(highlight) && lines.length <= highlightLimit;
  // The highlighted markup of each new text, computed once per file.
  const html = useMemo(() => {
    if (!canHighlight || !highlight) return null;
    return lines.map(l => (l.kind === 'context' || l.kind === 'added' || l.kind === 'changed') ? highlight(l.text, file.path) : null);
  }, [canHighlight, highlight, lines, file.path]);

  const inSelection = (index: number) =>
    selection?.path === file.path && index >= selection.start && index <= selection.end;
  const added = file.added ?? 0;
  const removed = file.removed ?? 0;

  const text = (index: number, value: string, kind: string, isNew: boolean, side?: 'old' | 'new') => {
    const markup = isNew ? html?.[index] : null;
    return markup != null
      ? <span className="ui-diff-cell ui-diff-text" data-kind={kind} data-side={side} data-selected={inSelection(index) ? 'true' : undefined} dangerouslySetInnerHTML={{ __html: markup }} />
      : <span className="ui-diff-cell ui-diff-text" data-kind={kind} data-side={side} data-selected={inSelection(index) ? 'true' : undefined}>{value}</span>;
  };
  const number = (n: number | undefined, kind: string, index: number | null, side?: 'old' | 'new') => (
    <span className="ui-diff-cell ui-diff-number" data-kind={kind} data-side={side} data-selected={index !== null && inSelection(index) ? 'true' : undefined}>
      {n ?? ''}
    </span>
  );
  const marker = (m: string, kind: string, index: number | null, side?: 'old' | 'new') => (
    <span className="ui-diff-cell ui-diff-marker" data-kind={kind} data-side={side} data-selected={index !== null && inSelection(index) ? 'true' : undefined}>{m}</span>
  );
  const toggle = (item: Indexed | undefined, side?: 'old' | 'new') => {
    if (!selectable) return null;
    if (!item) return <span className="ui-diff-cell ui-diff-select" data-side={side} />;
    // A removed line is named by its old number and says so: in a modified
    // hunk the removed and the added line often share a number.
    const removedLine = item.line.kind === 'removed';
    const n = removedLine
      ? item.line.oldNumber ?? item.index + 1
      : item.line.newNumber ?? item.line.oldNumber ?? item.index + 1;
    const on = inSelection(item.index);
    return (
      <span className="ui-diff-cell ui-diff-select" data-side={side} data-selected={on ? 'true' : undefined}>
        <button
          type="button"
          className="ui-diff-select-button"
          aria-label={t(removedLine ? 'ui.diff.selectRemovedLine' : 'ui.diff.selectLine', String(n))}
          aria-pressed={on}
          onClick={(e: MouseEvent) => onToggle(file.path, item.index, e.shiftKey)}
        />
      </span>
    );
  };
  const spacer = () => (selectable ? <span className="ui-diff-cell ui-diff-select" /> : null);
  const toolbarAfter = (index: number) =>
    selectionToolbar && selection?.path === file.path && selection.end === index
      ? <div className="ui-diff-selection-toolbar">{selectionToolbar}</div>
      : null;
  const metaRow = ({ line, index }: Indexed) => (
    <div key={`m${index}`} className="ui-diff-row ui-diff-row-full" data-kind="meta">
      <span className="ui-diff-cell ui-diff-meta-text">{line.text}</span>
    </div>
  );

  const unified = (items: Indexed[]) => items.map(item => {
    const { line, index } = item;
    if (line.kind === 'meta') return metaRow(item);
    const kind = line.kind;
    if (kind === 'changed') {
      return (
        <Fragment key={index}>
          <div className="ui-diff-row" data-kind="changed">
            {toggle(item)}
            {number(line.oldNumber, kind, index)}
            {number(undefined, kind, index)}
            {marker(DIFF_MARKERS.removed, kind, index)}
            {text(index, line.otherText ?? '', kind, false)}
          </div>
          <div className="ui-diff-row" data-kind="changed">
            {spacer()}
            {number(undefined, kind, index)}
            {number(line.newNumber, kind, index)}
            {marker(DIFF_MARKERS.added, kind, index)}
            {text(index, line.text, kind, true)}
          </div>
          {toolbarAfter(index)}
        </Fragment>
      );
    }
    return (
      <Fragment key={index}>
        <div className="ui-diff-row" data-kind={kind}>
          {toggle(item)}
          {number(kind === 'added' ? undefined : line.oldNumber, kind, index)}
          {number(kind === 'removed' ? undefined : line.newNumber, kind, index)}
          {marker(DIFF_MARKERS[kind], kind, index)}
          {text(index, line.text, kind, kind !== 'removed')}
        </div>
        {toolbarAfter(index)}
      </Fragment>
    );
  });

  const split = (items: Indexed[]) => splitRows(items).map(row => {
    if ('meta' in row) return metaRow(row.meta);
    const { left, right, shared } = row;
    const leftKind = left ? (left.line.kind === 'context' ? 'context' : left.line.kind === 'changed' ? 'changed' : 'removed') : 'empty';
    const rightKind = right ? (right.line.kind === 'context' ? 'context' : right.line.kind === 'changed' ? 'changed' : 'added') : 'empty';
    const leftText = left ? (left.line.kind === 'changed' ? left.line.otherText ?? '' : left.line.text) : '';
    const last = Math.max(left?.index ?? -1, right?.index ?? -1);
    return (
      <Fragment key={`${left?.index ?? ''}-${right?.index ?? ''}`}>
        <div className="ui-diff-row">
          {toggle(left, 'old')}
          {number(left?.line.oldNumber, leftKind, left?.index ?? null, 'old')}
          {marker(left ? (leftKind === 'context' ? DIFF_MARKERS.context : DIFF_MARKERS.removed) : '', leftKind, left?.index ?? null, 'old')}
          {/* A context line is the same text on both sides: highlighted on both. */}
          {left ? text(left.index, leftText, leftKind, leftKind === 'context', 'old') : <span className="ui-diff-cell ui-diff-text" data-kind="empty" data-side="old" />}
          {shared ? spacer() : toggle(right, 'new')}
          {number(right?.line.newNumber, rightKind, right?.index ?? null, 'new')}
          {marker(right ? (rightKind === 'context' ? DIFF_MARKERS.context : DIFF_MARKERS.added) : '', rightKind, right?.index ?? null, 'new')}
          {right ? text(right.index, right.line.text, rightKind, true, 'new') : <span className="ui-diff-cell ui-diff-text" data-kind="empty" data-side="new" />}
        </div>
        {toolbarAfter(last)}
      </Fragment>
    );
  });

  // Each hunk's first line index is the count of the lines before it.
  const starts = file.hunks.map((_, h) =>
    file.hunks.slice(0, h).reduce((sum, before) => sum + before.lines.length, 0));
  const hunks = file.hunks.map((hunk, h) => {
    const items = hunk.lines.map((line, j) => ({ line, index: starts[h] + j }));
    return (
      <Fragment key={h}>
        {hunk.header && <div className="ui-diff-hunk"><span className="ui-diff-hunk-header">{hunk.header}</span></div>}
        {layout === 'split' ? split(items) : unified(items)}
      </Fragment>
    );
  });

  return (
    <div className="ui-diff-file" role="group" aria-labelledby={pathId}>
      <div className="ui-diff-file-header">
        <span className="ui-diff-paths">
          {file.previousPath && (
            <>
              <span className="ui-visually-hidden">{t('ui.diff.previousPath')}</span>
              <span className="ui-diff-previous-path">{file.previousPath}</span>
              <span className="ui-diff-arrow" aria-hidden="true">→</span>
            </>
          )}
          <span className="ui-diff-path" id={pathId}>{file.path}</span>
        </span>
        <StatusBadge tone={STATUS_TONES[file.status]}>{t(`ui.diff.status.${file.status}`)}</StatusBadge>
        {!file.binary && (
          <span className="ui-diff-counts">
            <span className="ui-diff-count" data-kind="added" aria-hidden="true">+{added}</span>
            <span className="ui-diff-count" data-kind="removed" aria-hidden="true">−{removed}</span>
            <span className="ui-visually-hidden">{t('ui.diff.counts', String(added), String(removed))}</span>
          </span>
        )}
        {showLines && copyable && (
          <span className="ui-diff-file-actions">
            <CopyButton size="sm" label={t('ui.diff.copyFile', file.path)} getText={() => newText(file)} />
          </span>
        )}
      </div>
      {file.binary
        ? <p className="ui-diff-file-note">{t('ui.diff.binary')}</p>
        : file.truncated
          ? <p className="ui-diff-file-note">{t('ui.diff.truncated')}</p>
          : <div className="ui-diff-body" data-selectable={selectable ? 'true' : undefined}>{hunks}</div>}
    </div>
  );
}

/** KT-2004 — a diff, read from a model: unified or side by side (unified in
 *  a narrow container), a header per file, number gutters and a written
 *  +/−/space marker, hunk headers and meta lines, the diff tokens for colour,
 *  a caller's highlighter skipped above a line limit, wrap, copy per file, a
 *  line selection made of named toggle buttons, loading and empty states. */
export function DiffViewer({
  files, label, layout: initialLayout = 'unified', onLayoutChange, wrap: initialWrap = true, maxHeight, highlight,
  highlightLimit = DIFF_HIGHLIGHT_LIMIT, copyable = true, selectable = false, selection = null, onSelectionChange,
  maxSelection = DIFF_MAX_SELECTION, selectionToolbar, loading = false, loadingLabel, empty, className,
}: DiffViewerProps) {
  const { t } = useT();
  const rootRef = useRef<HTMLDivElement>(null);
  const narrow = useNarrow(rootRef);
  const [layout, setLayout] = useState<DiffLayout>(initialLayout);
  const [wrap, setWrap] = useState(initialWrap);
  const anchor = useRef<{ path: string; index: number } | null>(null);

  // A new `layout` or `wrap` prop resets the reader's choice, during render
  // (react.dev, "Adjusting some state when a prop changes"), not in an effect.
  const [prevLayout, setPrevLayout] = useState(initialLayout);
  if (prevLayout !== initialLayout) {
    setPrevLayout(initialLayout);
    setLayout(initialLayout);
  }
  const [prevWrap, setPrevWrap] = useState(initialWrap);
  if (prevWrap !== initialWrap) {
    setPrevWrap(initialWrap);
    setWrap(initialWrap);
  }

  const shown: DiffLayout = narrow ? 'unified' : layout;
  const pick = (next: DiffLayout) => {
    setLayout(next);
    if (next !== layout) onLayoutChange?.(next);
  };

  const onToggle = (path: string, index: number, extend: boolean) => {
    if (!onSelectionChange) return;
    const sameFile = selection?.path === path;
    if (extend && sameFile && selection) {
      const from = anchor.current?.path === path ? anchor.current.index : selection.start;
      const max = Math.max(1, maxSelection);
      const range = index >= from
        ? { start: from, end: Math.min(index, from + max - 1) }
        : { start: Math.max(index, from - max + 1), end: from };
      onSelectionChange({ path, ...range });
      return;
    }
    if (sameFile && selection && selection.start === index && selection.end === index) {
      anchor.current = null;
      onSelectionChange(null);
      return;
    }
    anchor.current = { path, index };
    onSelectionChange({ path, start: index, end: index });
  };

  const classes = ['ui-diff', className].filter(Boolean).join(' ');
  const bounded = maxHeight !== undefined;

  let content: ReactNode;
  if (loading) {
    content = <LoadingState size="sm" message={loadingLabel ?? t('ui.loading.label')} />;
  } else if (files.length === 0) {
    content = <p className="ui-diff-empty">{empty ?? t('ui.diff.empty')}</p>;
  } else {
    content = (
      <>
        <div className="ui-diff-toolbar">
          {!narrow && (
            <div className="ui-diff-layouts" role="group" aria-label={t('ui.diff.layout')}>
              {(['unified', 'split'] as const).map(option => (
                <Button
                  key={option}
                  size="sm"
                  variant={layout === option ? 'neutral' : 'ghost'}
                  aria-pressed={layout === option}
                  onClick={() => pick(option)}
                >
                  {t(`ui.diff.layout.${option}`)}
                </Button>
              ))}
            </div>
          )}
          <IconButton
            size="sm"
            label={t('ui.diff.wrap')}
            icon={<WrapText />}
            aria-pressed={wrap}
            onClick={() => setWrap(w => !w)}
            data-action="wrap"
          />
        </div>
        <div
          className="ui-diff-files"
          data-max-height={maxHeight}
          role={bounded ? 'region' : undefined}
          aria-label={bounded ? label ?? t('ui.diff.label') : undefined}
          tabIndex={bounded ? 0 : undefined}
        >
          {files.map((file, i) => (
            <DiffFile
              key={`${i}:${file.previousPath ?? ''}:${file.path}`}
              file={file}
              layout={shown}
              highlight={highlight}
              highlightLimit={highlightLimit}
              copyable={copyable}
              selectable={selectable}
              selection={selection}
              onToggle={onToggle}
              selectionToolbar={selectionToolbar}
            />
          ))}
        </div>
      </>
    );
  }

  return (
    <div
      ref={rootRef}
      className={classes}
      data-layout={shown}
      data-wrap={wrap ? 'true' : 'false'}
      data-narrow={narrow ? 'true' : undefined}
      aria-busy={loading || undefined}
    >
      {content}
    </div>
  );
}

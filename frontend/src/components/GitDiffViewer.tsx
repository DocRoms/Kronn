import { useState } from 'react';
import { Loader2, MessageSquare, X } from 'lucide-react';
import { highlightLine, languageForPath, parseDiffLines, type DiffLine } from '../lib/diff-syntax';

interface Props {
  path: string;
  content: string;
  loading: boolean;
  className?: string;
  /** Called with the selected contiguous range when the user confirms
   *  "Talk about it in the discussion". Omitted → the comment affordance
   *  (💬 per line, range selection) does not render at all, so a reused
   *  read-only viewer stays exactly as before. */
  onCommentSelection?: (selection: { lines: DiffLine[] }) => void;
  t?: (key: string, ...args: (string | number)[]) => string;
}

/** A selection can span at most this many diff lines. Keeps an injected
 *  `diff` block small enough to stay a quote, not a second copy of the file. */
export const MAX_COMMENT_SELECTION_LINES = 50;

const isCommentable = (line: DiffLine) =>
  line.kind === 'add' || line.kind === 'del' || line.kind === 'context';

interface SelectionRange {
  anchor: number;
  focus: number;
  truncated: boolean;
  // The target this selection was made against. Array indexes from a
  // different file (or a refreshed diff for the same file) mean nothing —
  // or worse, something wrong — so a stale selection is discarded by
  // comparing identity at render time instead of clearing it with an
  // effect: a click always replaces the whole object with the CURRENT
  // path/content, so the stale one is simply never read again.
  forPath: string;
  forContent: string;
}

/** Shared, syntax-aware Git diff renderer.
 * Kept independent from GitPanel so Project > Code can reuse the exact same
 * visualisation when its Git-diff mode is added. */
export function GitDiffViewer({ path, content, loading, className = '', onCommentSelection, t }: Props) {
  const translate = t ?? ((key: string) => key);
  const [rawRange, setRawRange] = useState<SelectionRange | null>(null);
  const range = rawRange && rawRange.forPath === path && rawRange.forContent === content ? rawRange : null;

  if (loading) {
    return (
      <div className={`git-diff-container ${className}`.trim()}>
        <div className="git-center">
          <Loader2 size={16} style={{ animation: 'spin 1s linear infinite' }} />
        </div>
      </div>
    );
  }

  const language = languageForPath(path);
  const parsed = parseDiffLines(content);

  const selectedIndexes = (() => {
    if (!range) return null;
    const [lo, hi] = range.anchor <= range.focus ? [range.anchor, range.focus] : [range.focus, range.anchor];
    return { lo, hi };
  })();

  // The nearest hunk/meta index strictly between `from` and `to` (both
  // inclusive of `to`, exclusive of `from`), walking in the given direction.
  // `null` when the whole span is commentable lines — a diff omits the
  // unchanged lines between hunks, so a selection that silently crossed one
  // would look contiguous while quoting source that isn't actually adjacent.
  const firstBoundary = (from: number, to: number, direction: 1 | -1): number | null => {
    for (let i = from + direction; i !== to + direction; i += direction) {
      const kind = parsed[i]?.kind;
      if (kind === 'hunk' || kind === 'meta') return i;
    }
    return null;
  };

  // A click on the anchor line clears the whole selection. A click anywhere
  // else always extends from the anchor to the clicked line — no modifier
  // key required to discover the gesture. Shift-click still lands here too
  // and behaves the same way, so nothing regresses for anyone already used
  // to it. The far end (`focus`) is clamped to MAX_COMMENT_SELECTION_LINES
  // and to the contiguous run of commentable lines (never crosses a
  // hunk/meta boundary) so a click across a huge file, or across two
  // separate hunks, can't inject a misleadingly "contiguous" block.
  const handleLineClick = (index: number) => {
    if (range && index === range.anchor) {
      setRawRange(null);
      return;
    }
    if (range) {
      const { anchor } = range;
      const direction = index >= anchor ? 1 : -1;
      const boundary = firstBoundary(anchor, index, direction);
      const requested = boundary === null ? index : boundary - direction;
      const focus = anchor <= requested
        ? Math.min(requested, anchor + MAX_COMMENT_SELECTION_LINES - 1)
        : Math.max(requested, anchor - MAX_COMMENT_SELECTION_LINES + 1);
      const truncated = focus !== index;
      setRawRange({ anchor, focus, truncated, forPath: path, forContent: content });
      return;
    }
    setRawRange({ anchor: index, focus: index, truncated: false, forPath: path, forContent: content });
  };

  const confirmSelection = () => {
    if (!selectedIndexes || !onCommentSelection) return;
    const lines = parsed.slice(selectedIndexes.lo, selectedIndexes.hi + 1);
    onCommentSelection({ lines });
    setRawRange(null);
  };

  return (
    <div className={`git-diff-container ${className}`.trim()}>
      {onCommentSelection && (
        selectedIndexes ? (
          <div className="git-diff-selection-bar" role="toolbar">
            <span>
              {translate('git.diffSelectionCount', selectedIndexes.hi - selectedIndexes.lo + 1)}
              {range?.truncated && ` · ${translate('git.diffSelectionTruncated')}`}
            </span>
            <button type="button" className="git-small-btn" onClick={confirmSelection}>
              <MessageSquare size={12} /> {translate('git.diffTalkAboutIt')}
            </button>
            <button
              type="button"
              className="git-icon-btn"
              onClick={() => setRawRange(null)}
              aria-label={translate('common.cancel')}
            >
              <X size={12} />
            </button>
          </div>
        ) : (
          <div className="git-diff-selection-tip">{translate('git.diffSelectionTip')}</div>
        )
      )}
      <pre className="git-diff-pre">
        {parsed.map((line, index) => {
          const selected = !!selectedIndexes && index >= selectedIndexes.lo && index <= selectedIndexes.hi;
          const commentButton = onCommentSelection && isCommentable(line) && (
            <button
              type="button"
              className="git-diff-comment-btn"
              data-selected={selected}
              aria-pressed={selected}
              aria-label={translate('git.diffCommentLine')}
              title={translate('git.diffCommentLineHint')}
              onClick={() => handleLineClick(index)}
            >
              <MessageSquare size={11} />
            </button>
          );

          if (line.kind === 'del') {
            return (
              <div key={index} className="git-diff-line git-diff-line-del" data-selected={selected}>
                {commentButton}
                <span className="git-diff-prefix">-</span>
                <span className="git-diff-content">{line.content || '\u00A0'}</span>
              </div>
            );
          }
          if (line.kind === 'hunk') {
            return (
              <div key={index} className="git-diff-line git-diff-line-hunk">
                <span className="git-diff-content">{line.raw}</span>
              </div>
            );
          }
          if (line.kind === 'meta') {
            return (
              <div key={index} className="git-diff-line git-diff-line-meta">
                <span className="git-diff-content">{line.raw || '\u00A0'}</span>
              </div>
            );
          }

          const prefix = line.kind === 'add' ? '+' : ' ';
          const kindClass = line.kind === 'add' ? 'git-diff-line-add' : 'git-diff-line-ctx';
          const html = highlightLine(line.content, language);
          return (
            <div key={index} className={`git-diff-line ${kindClass}`} data-selected={selected}>
              {commentButton}
              <span className="git-diff-prefix">{prefix}</span>
              <span
                className="git-diff-content hljs"
                dangerouslySetInnerHTML={{ __html: html || '\u00A0' }}
              />
            </div>
          );
        })}
      </pre>
    </div>
  );
}

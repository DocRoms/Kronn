/**
 * KT-453/461 — line/range "comment on this" selection in the Git diff
 * viewer. No modifier key is required to discover the gesture (a junior dev
 * shouldn't need to know Shift-click): a plain click on any other line now
 * extends the range exactly like Shift-click always did, and Shift-click
 * itself keeps working — nothing regresses for anyone who already knew it.
 * Clicking the anchor line again clears the whole selection. Pins:
 *  - no comment affordance at all when `onCommentSelection` is omitted
 *    (a reused read-only viewer must render exactly as before)
 *  - a visible tip explains the gesture before anything is selected
 *  - a plain click selects exactly one line and reports it
 *  - a plain click on another line extends a contiguous range from the
 *    fixed anchor, same as Shift-click
 *  - Shift-click still extends the range too — no regression
 *  - clicking the anchor again clears the whole selection
 *  - a range is clamped to MAX_COMMENT_SELECTION_LINES lines
 *  - a selection never crosses a hunk boundary
 *  - the confirm/cancel toolbar only appears while a selection is active,
 *    and cancel clears it without calling back
 *  - every comment button carries a real accessible name and pressed
 *    state (not color-only)
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { GitDiffViewer, MAX_COMMENT_SELECTION_LINES } from '../GitDiffViewer';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

const SAMPLE_DIFF = [
  'diff --git a/src/main.rs b/src/main.rs',
  'index abc..def 100644',
  '--- a/src/main.rs',
  '+++ b/src/main.rs',
  '@@ -1,3 +1,4 @@',
  ' fn main() {',
  '-    let x = 1;',
  '+    let x = 2;',
  '+    let y = 3;',
  ' }',
].join('\n');

function commentButtons() {
  return screen.getAllByLabelText('git.diffCommentLine');
}

function renderViewer(content: string, onCommentSelection = vi.fn(), path = 'src/main.rs') {
  render(
    <GitDiffViewer path={path} content={content} loading={false} t={t} onCommentSelection={onCommentSelection} />,
  );
  return onCommentSelection;
}

describe('GitDiffViewer — comment-on-line/-range (KT-453/461)', () => {
  it('renders no comment affordance when onCommentSelection is omitted', () => {
    render(<GitDiffViewer path="src/main.rs" content={SAMPLE_DIFF} loading={false} t={t} />);
    expect(screen.queryAllByLabelText('git.diffCommentLine')).toHaveLength(0);
  });

  it('shows a visible tip before anything is selected — not just a hover tooltip', () => {
    renderViewer(SAMPLE_DIFF);
    expect(screen.getByText('git.diffSelectionTip')).toBeDefined();
    expect(screen.queryByText(/git.diffSelectionCount/)).toBeNull();
  });

  it('a plain click selects exactly one line and confirming reports it', () => {
    const onCommentSelection = renderViewer(SAMPLE_DIFF);
    // 5 commentable lines: context, del, add, add, context.
    const buttons = commentButtons();
    expect(buttons).toHaveLength(5);

    fireEvent.click(buttons[1]!); // the `-    let x = 1;` line
    expect(screen.getByText('git.diffSelectionCount(1)')).toBeDefined();
    // The tip is replaced by the action bar once something is selected.
    expect(screen.queryByText('git.diffSelectionTip')).toBeNull();

    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));
    expect(onCommentSelection).toHaveBeenCalledTimes(1);
    const [{ lines }] = onCommentSelection.mock.calls[0]!;
    expect(lines).toHaveLength(1);
    expect(lines[0].kind).toBe('del');
    expect(lines[0].content).toBe('    let x = 1;');

    // Selection clears after confirming — the tip is back.
    expect(screen.queryByText(/git.diffSelectionCount/)).toBeNull();
    expect(screen.getByText('git.diffSelectionTip')).toBeDefined();
  });

  it('a plain click on another line extends the range from the fixed anchor — no modifier needed', () => {
    const onCommentSelection = renderViewer(SAMPLE_DIFF);
    const buttons = commentButtons();
    fireEvent.click(buttons[0]!); // ` fn main() {`
    fireEvent.click(buttons[3]!); // extend to `+    let y = 3;`, no shiftKey
    expect(screen.getByText('git.diffSelectionCount(4)')).toBeDefined();

    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));
    const [{ lines }] = onCommentSelection.mock.calls[0]!;
    expect(lines.map((l: { kind: string }) => l.kind)).toEqual(['context', 'del', 'add', 'add']);
  });

  it('Shift-click still extends the range too — no regression for those who already knew it', () => {
    const onCommentSelection = renderViewer(SAMPLE_DIFF);
    const buttons = commentButtons();
    fireEvent.click(buttons[0]!);
    fireEvent.click(buttons[3]!, { shiftKey: true });
    expect(screen.getByText('git.diffSelectionCount(4)')).toBeDefined();

    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));
    const [{ lines }] = onCommentSelection.mock.calls[0]!;
    expect(lines.map((l: { kind: string }) => l.kind)).toEqual(['context', 'del', 'add', 'add']);
  });

  it('extending backward keeps the original anchor, not the far end', () => {
    const onCommentSelection = renderViewer(SAMPLE_DIFF);
    const buttons = commentButtons();
    fireEvent.click(buttons[3]!); // anchor = `+    let y = 3;`
    fireEvent.click(buttons[0]!); // extend up to ` fn main() {`
    expect(screen.getByText('git.diffSelectionCount(4)')).toBeDefined();

    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));
    const [{ lines }] = onCommentSelection.mock.calls[0]!;
    // Anchor (index 3) must still be included in the range.
    expect(lines.map((l: { kind: string }) => l.kind)).toEqual(['context', 'del', 'add', 'add']);
  });

  it('clicking the anchor line again clears the whole selection', () => {
    const onCommentSelection = renderViewer(SAMPLE_DIFF);
    const buttons = commentButtons();
    fireEvent.click(buttons[1]!); // anchor
    fireEvent.click(buttons[3]!); // extend
    expect(screen.getByText('git.diffSelectionCount(3)')).toBeDefined();

    fireEvent.click(buttons[1]!); // re-click the anchor
    expect(screen.queryByText(/git.diffSelectionCount/)).toBeNull();
    expect(screen.getByText('git.diffSelectionTip')).toBeDefined();
    expect(onCommentSelection).not.toHaveBeenCalled();
  });

  it('clamps the range to MAX_COMMENT_SELECTION_LINES', () => {
    const bigDiff = [
      '@@ -1,60 +1,60 @@',
      ...Array.from({ length: 60 }, (_, i) => ` line ${i}`),
    ].join('\n');
    const onCommentSelection = renderViewer(bigDiff, vi.fn(), 'src/big.txt');
    const buttons = commentButtons();
    expect(buttons).toHaveLength(60);

    fireEvent.click(buttons[0]!);
    fireEvent.click(buttons[59]!);

    // The clamp is silent by count alone — it must also say so explicitly,
    // since "50 selected" reads as "exactly what I clicked" otherwise.
    expect(
      screen.getByText(`git.diffSelectionCount(${MAX_COMMENT_SELECTION_LINES}) · git.diffSelectionTruncated`),
    ).toBeDefined();
    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));
    const [{ lines }] = onCommentSelection.mock.calls[0]!;
    expect(lines).toHaveLength(MAX_COMMENT_SELECTION_LINES);
  });

  it('clears the selection when the diff path or content changes', () => {
    const onCommentSelection = vi.fn();
    const { rerender } = render(
      <GitDiffViewer
        path="src/main.rs"
        content={SAMPLE_DIFF}
        loading={false}
        t={t}
        onCommentSelection={onCommentSelection}
      />,
    );
    fireEvent.click(commentButtons()[0]!);
    expect(screen.getByText('git.diffSelectionCount(1)')).toBeDefined();

    // Switching to a different file's diff must not leave a stale,
    // now-meaningless index-based selection active.
    rerender(
      <GitDiffViewer
        path="src/other.rs"
        content={SAMPLE_DIFF}
        loading={false}
        t={t}
        onCommentSelection={onCommentSelection}
      />,
    );
    expect(screen.queryByText(/git.diffSelectionCount/)).toBeNull();
  });

  it('also clears the selection when the path is unchanged but the content is refreshed', () => {
    const onCommentSelection = vi.fn();
    const { rerender } = render(
      <GitDiffViewer
        path="src/main.rs"
        content={SAMPLE_DIFF}
        loading={false}
        t={t}
        onCommentSelection={onCommentSelection}
      />,
    );
    fireEvent.click(commentButtons()[0]!);
    expect(screen.getByText('git.diffSelectionCount(1)')).toBeDefined();

    // Same file, but a refetch (e.g. after a commit) returns different diff
    // text — the old index-based selection is just as stale here.
    const refreshedDiff = `${SAMPLE_DIFF}\n+one more line`;
    rerender(
      <GitDiffViewer
        path="src/main.rs"
        content={refreshedDiff}
        loading={false}
        t={t}
        onCommentSelection={onCommentSelection}
      />,
    );
    expect(screen.queryByText(/git.diffSelectionCount/)).toBeNull();
  });

  it('never extends a selection across a hunk boundary, even via a far click', () => {
    const twoHunks = [
      '@@ -1,2 +1,2 @@',
      ' context-a',
      '+added-a',
      '@@ -20,2 +20,2 @@',
      ' context-b',
      '+added-b',
    ].join('\n');
    const onCommentSelection = renderViewer(twoHunks);
    const buttons = commentButtons();
    expect(buttons).toHaveLength(4); // context-a, added-a, context-b, added-b

    fireEvent.click(buttons[0]!); // context-a
    fireEvent.click(buttons[3]!); // click added-b, past the 2nd `@@`, no shiftKey needed

    // Stops at the end of the first hunk — never swallows the `@@` header
    // or jumps into the unrelated second hunk.
    expect(
      screen.getByText('git.diffSelectionCount(2) · git.diffSelectionTruncated'),
    ).toBeDefined();

    fireEvent.click(screen.getByText('git.diffTalkAboutIt'));
    const [{ lines }] = onCommentSelection.mock.calls[0]!;
    expect(lines.map((line: { content: string }) => line.content)).toEqual(['context-a', 'added-a']);
  });

  it('cancel clears the selection without calling back', () => {
    const onCommentSelection = renderViewer(SAMPLE_DIFF);
    fireEvent.click(commentButtons()[0]!);
    expect(screen.getByText('git.diffSelectionCount(1)')).toBeDefined();

    fireEvent.click(screen.getByLabelText('common.cancel'));
    expect(screen.queryByText(/git.diffSelectionCount/)).toBeNull();
    expect(onCommentSelection).not.toHaveBeenCalled();
  });

  it('every comment button carries an accessible name and pressed state, not color-only', () => {
    renderViewer(SAMPLE_DIFF);
    const buttons = commentButtons();
    for (const button of buttons) {
      expect(button.getAttribute('aria-label')).toBe('git.diffCommentLine');
      expect(button.getAttribute('aria-pressed')).toBe('false');
    }
    fireEvent.click(buttons[0]!);
    expect(buttons[0]!.getAttribute('aria-pressed')).toBe('true');
  });
});

/**
 * Swipe-gesture coverage for SwipeableDiscItem.
 *
 * The `unread.test.tsx` sibling pins the `unseenBasis` badge contract; this
 * file covers the previously-uncovered pointer/swipe machinery:
 *  - pointerDown → pointerMove → pointerUp drives the gesture
 *  - delta clamping: sign(delta) * min(|delta| * 0.7, 120)
 *  - threshold crossing (|offsetX| > 80) fires archive (right) / delete (left)
 *  - short swipe back under threshold snaps back, fires nothing destructive
 *  - a tap (|offsetX| < 5) fires onSelect
 *  - pointerCancel aborts the gesture
 *  - setPointerCapture is invoked on the target
 *
 * happy-dom doesn't ship a PointerEvent constructor by default; @testing-library's
 * fireEvent.pointerDown/Move/Up still dispatch the right React synthetic events,
 * and we feed `clientX` deltas directly. We also stub `setPointerCapture` on the
 * Element prototype since happy-dom doesn't implement it.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';

// Avoid pulling the real I18nProvider (which hits configApi on mount). The
// component only reads `locale` off useT(); a static stub is enough.
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ locale: 'fr', setLocale: () => {}, t: (k: string) => k }),
}));

import { SwipeableDiscItem } from '../SwipeableDiscItem';
import type { Discussion } from '../../types/generated';

const skel = {
  id: 'disc-alpha', project_id: null, title: 'DiscAlpha', agent: 'ClaudeCode' as const,
  language: 'fr', participants: [], archived: false, pinned: false,
  workspace_mode: 'Direct', created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
} satisfies Partial<Discussion>;

function disc(p: Partial<Discussion> = {}): Discussion {
  return { ...skel, messages: [], message_count: 0, non_system_message_count: 0, ...p } as Discussion;
}

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

function renderItem(props?: Partial<Parameters<typeof SwipeableDiscItem>[0]>) {
  const onSelect = vi.fn();
  const onArchive = vi.fn();
  const onDelete = vi.fn();
  const onStop = vi.fn();
  render(
    <SwipeableDiscItem
      disc={disc()}
      isActive={false}
      lastSeenCount={0}
      isSending={false}
      onSelect={onSelect}
      onArchive={onArchive}
      onDelete={onDelete}
      onStop={onStop}
      t={t}
      {...props}
    />,
  );
  // The labelled open button owns keyboard selection + pointer gestures; row
  // actions (including the KT-464 favorite toggle, whose label also carries
  // "DiscAlpha") are sibling buttons and must never steal this query — the
  // open button's accessible name is the only one with the em dash + count.
  const row = screen.getByRole('button', { name: /DiscAlpha — \d+ messages/ });
  return { onSelect, onArchive, onDelete, onStop, row };
}

/** Read the live translateX(...) value off the inline style. */
function offsetOf(row: HTMLElement): number {
  const transformed = row.closest('.disc-item') as HTMLElement | null;
  const m = /translateX\((-?\d+(?:\.\d+)?)px\)/.exec(transformed?.style.transform ?? '');
  return m ? Number(m[1]) : NaN;
}

beforeEach(() => {
  // happy-dom doesn't implement pointer capture; the handler calls it
  // unconditionally on pointerdown.
  if (!('setPointerCapture' in Element.prototype)) {
    // @ts-expect-error — augmenting the prototype for the test env
    Element.prototype.setPointerCapture = function () {};
  }
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('SwipeableDiscItem — pointer/swipe gesture', () => {
  it('invokes setPointerCapture on pointerDown', () => {
    const spy = vi.spyOn(Element.prototype, 'setPointerCapture');
    const { row } = renderItem();
    fireEvent.pointerDown(row, { clientX: 100, pointerId: 1 });
    expect(spy).toHaveBeenCalledWith(1);
    spy.mockRestore();
  });

  it('clamps the move delta to sign(delta) * min(|delta| * 0.7, 120)', () => {
    const { row } = renderItem();
    fireEvent.pointerDown(row, { clientX: 0, pointerId: 1 });

    // Small move: 40px * 0.7 = 28px, under the 120 cap.
    fireEvent.pointerMove(row, { clientX: 40, pointerId: 1 });
    expect(offsetOf(row)).toBeCloseTo(28, 5);

    // Large move: 500px * 0.7 = 350 → clamped to 120.
    fireEvent.pointerMove(row, { clientX: 500, pointerId: 1 });
    expect(offsetOf(row)).toBe(120);

    // Large negative move clamps to -120.
    fireEvent.pointerMove(row, { clientX: -500, pointerId: 1 });
    expect(offsetOf(row)).toBe(-120);
  });

  it('right swipe past threshold fires onArchive (not delete/select)', () => {
    const { row, onArchive, onDelete, onSelect } = renderItem();
    fireEvent.pointerDown(row, { clientX: 0, pointerId: 1 });
    // Need offsetX > 80. clamped = min(|d|*0.7, 120). |d| >= 115 → offset 80.5+.
    fireEvent.pointerMove(row, { clientX: 200, pointerId: 1 }); // 140 → 120
    fireEvent.pointerUp(row, { clientX: 200, pointerId: 1 });

    expect(onArchive).toHaveBeenCalledWith('disc-alpha');
    expect(onDelete).not.toHaveBeenCalled();
    expect(onSelect).not.toHaveBeenCalled();
    // Row snaps back to 0 after release.
    expect(offsetOf(row)).toBe(0);
  });

  it('left swipe past threshold fires onDelete (not archive/select)', () => {
    const { row, onArchive, onDelete, onSelect } = renderItem();
    fireEvent.pointerDown(row, { clientX: 0, pointerId: 1 });
    fireEvent.pointerMove(row, { clientX: -200, pointerId: 1 }); // -140 → -120
    fireEvent.pointerUp(row, { clientX: -200, pointerId: 1 });

    expect(onDelete).toHaveBeenCalledWith('disc-alpha');
    expect(onArchive).not.toHaveBeenCalled();
    expect(onSelect).not.toHaveBeenCalled();
    expect(offsetOf(row)).toBe(0);
  });

  it('swipe not far enough snaps back and fires nothing destructive', () => {
    const { row, onArchive, onDelete, onSelect } = renderItem();
    fireEvent.pointerDown(row, { clientX: 0, pointerId: 1 });
    // 60px * 0.7 = 42 → under the 80 threshold, over the 5 tap window.
    fireEvent.pointerMove(row, { clientX: 60, pointerId: 1 });
    expect(offsetOf(row)).toBeCloseTo(42, 5);
    fireEvent.pointerUp(row, { clientX: 60, pointerId: 1 });

    expect(onArchive).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
    expect(onSelect).not.toHaveBeenCalled(); // 42 is outside the |offset| < 5 tap window
    expect(offsetOf(row)).toBe(0);
  });

  it('a tap (no/tiny movement) fires onSelect with the unseen basis', () => {
    const { row, onSelect, onArchive, onDelete } = renderItem({
      disc: disc({ non_system_message_count: 4, message_count: 9 }),
    });
    fireEvent.pointerDown(row, { clientX: 100, pointerId: 1 });
    fireEvent.pointerUp(row, { clientX: 100, pointerId: 1 });

    // unseenBasis prefers non_system_message_count (4) over message_count (9).
    expect(onSelect).toHaveBeenCalledWith('disc-alpha', 4);
    expect(onArchive).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
  });

  it('pointerUp without a prior pointerDown is a no-op (swiping guard)', () => {
    const { row, onSelect, onArchive, onDelete } = renderItem();
    fireEvent.pointerUp(row, { clientX: 100, pointerId: 1 });
    expect(onSelect).not.toHaveBeenCalled();
    expect(onArchive).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
  });

  it('pointerMove without pointerDown does not move the row (swiping guard)', () => {
    const { row } = renderItem();
    fireEvent.pointerMove(row, { clientX: 300, pointerId: 1 });
    expect(offsetOf(row)).toBe(0);
  });

  it('pointerCancel aborts the gesture and resets the offset', () => {
    const { row, onArchive, onDelete, onSelect } = renderItem();
    fireEvent.pointerDown(row, { clientX: 0, pointerId: 1 });
    fireEvent.pointerMove(row, { clientX: 200, pointerId: 1 }); // would be archive
    fireEvent.pointerCancel(row, { clientX: 200, pointerId: 1 });

    // No callback fired; offset reset.
    expect(onArchive).not.toHaveBeenCalled();
    expect(onDelete).not.toHaveBeenCalled();
    expect(onSelect).not.toHaveBeenCalled();
    expect(offsetOf(row)).toBe(0);

    // And a subsequent pointerUp (swiping now false) is inert.
    fireEvent.pointerUp(row, { clientX: 200, pointerId: 1 });
    expect(onArchive).not.toHaveBeenCalled();
  });

  it('exposes the directional swipe label + background while swiping right', () => {
    const { row } = renderItem({ archiveLabel: 'disc.archive' });
    fireEvent.pointerDown(row, { clientX: 0, pointerId: 1 });
    fireEvent.pointerMove(row, { clientX: 200, pointerId: 1 });
    // label is shown once offsetX > 30 (right → archive label)
    expect(screen.getByText('disc.archive')).toBeDefined();
  });

  it('shows the delete label while swiping left past the reveal point', () => {
    const { row } = renderItem();
    fireEvent.pointerDown(row, { clientX: 0, pointerId: 1 });
    fireEvent.pointerMove(row, { clientX: -200, pointerId: 1 });
    expect(screen.getByText('disc.delete')).toBeDefined();
  });
});

describe('SwipeableDiscItem — compact state cluster', () => {
  it('keeps worktree and shared-room indicators outside the title text', () => {
    renderItem({
      disc: disc({
        workspace_mode: 'Isolated',
        shared_id: 'shared-1',
        participants: ['Codex', 'ClaudeCode'],
      }),
    });

    const titleText = document.querySelector('.disc-item-title-text')!;
    const cluster = document.querySelector('.disc-item-state-cluster')!;
    expect(cluster).not.toBeNull();
    expect(titleText.contains(cluster)).toBe(false);
    expect(screen.getByLabelText('disc.workspaceIsolated')).toBeInTheDocument();
    expect(screen.getByLabelText('disc.sidebar.sharedDiscussion')).toBeInTheDocument();
    expect(screen.queryByLabelText('disc.sidebar.multiAgentDiscussion')).toBeNull();
  });

  it('labels a local discussion with several agents without calling it shared', () => {
    renderItem({
      disc: disc({
        participants: ['Codex', 'ClaudeCode'],
        shared_id: null,
      }),
    });

    expect(screen.getByLabelText('disc.sidebar.multiAgentDiscussion')).toBeInTheDocument();
    expect(screen.queryByLabelText('disc.sidebar.sharedDiscussion')).toBeNull();
  });
});

describe('SwipeableDiscItem — row action menu', () => {
  it('keeps actions outside the discussion-open button and toggles a favorite (KT-464)', () => {
    const onTogglePin = vi.fn();
    const { row: open } = renderItem({ onTogglePin });

    // KT-464 — a visible star sits next to the "⋯" menu, same contract as
    // the Automations/Pages row favorites; it's no longer a menu item.
    const favorite = screen.getByRole('button', { name: 'disc.pin · DiscAlpha' });
    const actions = screen.getByRole('button', { name: 'disc.actions' });
    expect(open.contains(favorite)).toBe(false);
    expect(open.contains(actions)).toBe(false);
    expect(favorite.getAttribute('aria-pressed')).toBe('false');

    fireEvent.click(favorite);
    expect(onTogglePin).toHaveBeenCalledWith('disc-alpha', true);

    // The "⋯" menu no longer duplicates the favorite action.
    fireEvent.click(actions);
    expect(screen.queryByRole('menuitem', { name: /pin/i })).toBeNull();
  });

  it('renders no favorite toggle when onTogglePin is omitted', () => {
    renderItem();
    expect(screen.queryByRole('button', { name: /disc\.pin|disc\.unpin/ })).toBeNull();
  });

  it('reflects a pinned discussion as already-active and offers to unpin it', () => {
    const onTogglePin = vi.fn();
    renderItem({ onTogglePin, disc: disc({ pinned: true }) });

    const favorite = screen.getByRole('button', { name: 'disc.unpin · DiscAlpha' });
    expect(favorite.getAttribute('aria-pressed')).toBe('true');

    fireEvent.click(favorite);
    expect(onTogglePin).toHaveBeenCalledWith('disc-alpha', false);
  });

  it('archives from the explicit keyboard-accessible menu', () => {
    const { onArchive } = renderItem();
    fireEvent.click(screen.getByRole('button', { name: 'disc.actions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'disc.archive' }));
    expect(onArchive).toHaveBeenCalledWith('disc-alpha');
  });

  it('labels the copy action explicitly as an ID copy', () => {
    renderItem();
    fireEvent.click(screen.getByRole('button', { name: 'disc.actions' }));
    expect(screen.getByRole('menuitem', { name: 'disc.copyId' })).toBeInTheDocument();
  });
});

describe('SwipeableDiscItem — visible "N msg" label (non-System count)', () => {
  it('renders non_system_message_count, not the System-inflated message_count', () => {
    // Real shape: 2 user-facing msgs + 50 tool/refusal/summary System rows.
    renderItem({ disc: disc({ message_count: 52, non_system_message_count: 2 }) });
    expect(screen.getByText(/^2 msg ·/)).toBeDefined();
    expect(screen.queryByText(/52 msg/)).toBeNull();
    // aria-label matches too.
    expect(screen.getByRole('button', { name: /DiscAlpha/ }).getAttribute('aria-label'))
      .toContain('2 messages');
  });
});

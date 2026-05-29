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
  // The swipeable row is the role="button" element carrying the pointer handlers.
  const row = screen.getByRole('button');
  return { onSelect, onArchive, onDelete, onStop, row };
}

/** Read the live translateX(...) value off the inline style. */
function offsetOf(row: HTMLElement): number {
  const m = /translateX\((-?\d+(?:\.\d+)?)px\)/.exec(row.style.transform);
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

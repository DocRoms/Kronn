// LivePageActionOverlay — the card's width must not depend on where you clicked.
//
// Reported from real use on the "Suivi de la team" page: a `data-kronn-action`
// CTA at the END of a row opened a card a few dozen pixels wide, while the same
// CTA on the left opened a normal one. The box was absolutely positioned with
// `left` set inline here and `right: 8px` in the stylesheet, and no width — so
// its width was the space between the two offsets, i.e. a function of the click
// point. Measured in Chromium on an isolated fixture: 720px at anchor 8,
// 292px at 700, 42px at 950, and 0px once the shell was 600px wide.
//
// jsdom performs no layout, so these tests pin the CONTRACT that makes the
// layout correct rather than the pixels: the anchor is passed as a preference
// the stylesheet can clamp, never as a hard `left`, and the stylesheet decides
// the width before it looks at the anchor.

import { describe, expect, it, vi } from 'vitest';

// The card's own rendering is covered by LivePageActionCard.test.tsx. This file
// is about where the box sits and how wide it is, so the card is a stub — a
// thin fixture would otherwise fail inside the real card and say nothing about
// positioning.
vi.mock('../LivePageActionCard', () => ({
  LivePageActionCard: () => <div data-testid="card" />,
}));
import { readFileSync } from 'node:fs';
import { fireEvent, render, screen } from '@testing-library/react';
import { LivePageActionOverlay } from '../LivePageActionOverlay';
import type { LivePageAction } from '../../types/generated';
import type { LivePageActiveActionState } from '../../hooks/useLivePageActions';

const CSS = readFileSync('src/components/LivePageActionOverlay.css', 'utf8');

const action = { id: 'act-1', label: 'Framer', kind: 'prompt' } as unknown as LivePageAction;

function activeAt(left: number, slot = false): LivePageActiveActionState {
  return {
    activation: 1,
    bindings: {},
    anchor: { left, top: 100, width: 80, height: 24, slot },
  } as unknown as LivePageActiveActionState;
}

function overlayAt(left: number, slot = false): HTMLElement {
  const { container } = render(
    <LivePageActionOverlay
      active={activeAt(left, slot)}
      action={action}
      onChanged={() => {}}
      onClose={() => {}}
      onOpenDiscussion={() => {}}
    />,
  );
  const overlay = container.querySelector<HTMLElement>('.live-page-action-overlay');
  if (!overlay) throw new Error('the overlay did not render');
  return overlay;
}

describe('LivePageActionOverlay', () => {
  it('passes the anchor as a variable the stylesheet can clamp, not as a hard left', () => {
    const overlay = overlayAt(950);

    expect(overlay.style.getPropertyValue('--kr-action-anchor-left')).toBe('950px');
    // The inline `left` is what co-constrained the box against the CSS `right`
    // and crushed it. It must not come back.
    expect(overlay.style.left).toBe('');
  });

  it('sits under the row until the Page has opened its collapse', () => {
    // The frame between the click and the slot: the card goes just below the
    // row, which is where the slot is about to appear, so nothing jumps.
    expect(overlayAt(950).style.top).toBe('128px'); // top 100 + height 24 + 4
  });

  it('fills the collapse once the Page has opened one', () => {
    // With a slot the rectangle IS the gap: taking its top again would push
    // the card a full card-height below the hole it is meant to fill.
    expect(overlayAt(950, true).style.top).toBe('100px');
  });

  it('takes the row width, so the card is as wide as what it acts on', () => {
    expect(overlayAt(950).style.getPropertyValue('--kr-action-anchor-width')).toBe('80px');
  });

  it('reports its height so the Page can size the collapse, and clears it on close', () => {
    const onHeightChange = vi.fn();
    const { unmount } = render(
      <LivePageActionOverlay
        active={activeAt(12)}
        action={action}
        onChanged={() => {}}
        onClose={() => {}}
        onOpenDiscussion={() => {}}
        onHeightChange={onHeightChange}
      />,
    );
    expect(onHeightChange).toHaveBeenCalled();
    onHeightChange.mockClear();
    unmount();
    // A card that vanishes without saying so would leave a hole in the Page.
    expect(onHeightChange).toHaveBeenCalledWith(null);
  });

  it('never lets a near-zero anchor push the card off the left edge', () => {
    expect(overlayAt(-40).style.getPropertyValue('--kr-action-anchor-left')).toBe('8px');
  });

  it('decides the width before it looks at the anchor', () => {
    // The two halves of the original defect, pinned in the stylesheet:
    // a `right` alongside an inline `left` and no `width`.
    expect(CSS).toMatch(/width:\s*var\(--kr-action-card-width\)/);
    expect(CSS).not.toMatch(/^\s*right:/m);
    // `max-width` was the failed attempt: a ceiling can never be a floor.
    expect(CSS).not.toMatch(/max-width:\s*720px/);
    // And the clamp that lets the card slide instead of shrink.
    expect(CSS).toMatch(/left:\s*clamp\(/);
    // In a Page the card fills the collapse, so it drops its own 720px cap.
    expect(CSS).toMatch(/\.live-page-action-overlay \.discussion-action-card \{[^}]*width:\s*100%/);
  });

  it('renders nothing without an active action', () => {
    const { container } = render(
      <LivePageActionOverlay active={null} action={action} onChanged={() => {}}
      onClose={() => {}} onOpenDiscussion={() => {}} />,
    );
    expect(container.querySelector('.live-page-action-overlay')).toBeNull();
  });

  it('keeps a finished launch one click away from the fresh offer', () => {
    const previous = { id: 'launch-1', state: 'succeeded' } as unknown as LivePageAction;
    const onChanged = vi.fn();
    render(
      <LivePageActionOverlay
        active={{ ...activeAt(8), previous }}
        action={action}
        onChanged={onChanged}
        onClose={() => {}}
        onOpenDiscussion={() => {}}
      />,
    );
    fireEvent.click(screen.getByTestId('page-action-overlay-previous'));
    expect(onChanged).toHaveBeenCalledWith(previous, 1);
  });

  it('shows no previous-launch link on a running card', () => {
    const previous = { id: 'launch-1', state: 'succeeded' } as unknown as LivePageAction;
    render(
      <LivePageActionOverlay
        active={{ ...activeAt(8), card: previous, previous: undefined }}
        action={action}
        onChanged={() => {}}
        onClose={() => {}}
        onOpenDiscussion={() => {}}
      />,
    );
    expect(screen.queryByTestId('page-action-overlay-previous')).toBeNull();
  });
});

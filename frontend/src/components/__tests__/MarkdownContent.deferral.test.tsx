// Opening a 2 170-message room parsed 2 170 markdown documents in one
// synchronous pass and froze the tab. Neither the data nor the query was the
// cost — 2 MB total, the backend answering in under 10 ms — it was the parsing,
// all of it up front.
//
// The fix is a startup optimisation, not a window: every message still mounts
// and keeps its text in the DOM, so in-room search, Cmd+F and every "jump to
// this message" path are untouched. These tests pin both halves — that the
// deferral happens, and that what it defers to still holds the text.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';
import { MarkdownContent } from '../MessageBubble';
import { resetMarkdownUpgradeQueueForTests } from '../../lib/markdownUpgradeQueue';

const MD = 'Une réponse avec **du gras**.';

let idleCallbacks: ((d: { timeRemaining: () => number; didTimeout: boolean }) => void)[] = [];
let observed: Element[] = [];
let trigger: ((entries: { isIntersecting: boolean }[]) => void) | null = null;

function enableDeferral() {
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      constructor(cb: (entries: { isIntersecting: boolean }[]) => void) {
        trigger = cb;
      }
      observe(element: Element) { observed.push(element); }
      disconnect() {}
      unobserve() {}
    },
  );
  vi.stubGlobal('requestIdleCallback', (cb: (d: never) => void) => {
    idleCallbacks.push(cb as never);
    return idleCallbacks.length;
  });
}

function drainIdle() {
  act(() => {
    while (idleCallbacks.length > 0) {
      idleCallbacks.shift()!({ timeRemaining: () => 50, didTimeout: false });
    }
  });
}

describe('MarkdownContent deferral', () => {
  beforeEach(() => {
    resetMarkdownUpgradeQueueForTests();
    idleCallbacks = [];
    observed = [];
    trigger = null;
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('renders markdown immediately when the browser cannot defer', () => {
    // jsdom has neither observer. Degrading the page is never the safer
    // default, so the absence of the tools must mean "render now".
    render(<MarkdownContent content={MD} />);
    expect(screen.getByText('du gras').tagName).toBe('STRONG');
  });

  it('shows the text first and the markdown after an idle slice', () => {
    enableDeferral();
    const { container } = render(<MarkdownContent content={MD} />);

    expect(container.querySelector('.disc-md-deferred')).not.toBeNull();
    expect(container.querySelector('strong')).toBeNull();

    drainIdle();

    expect(container.querySelector('.disc-md-deferred')).toBeNull();
    expect(screen.getByText('du gras').tagName).toBe('STRONG');
  });

  it('keeps the message findable while it is still deferred', () => {
    // This is the whole reason the placeholder is the text and not a skeleton:
    // in-room search scans the DOM for `[data-message-search-content]`, and a
    // message rendering nothing would be reported as "no result" while being
    // right there. A window would have had exactly that defect.
    enableDeferral();
    const { container } = render(<MarkdownContent content="le mot introuvable" />);

    expect(container.querySelector('.disc-md-deferred')?.textContent).toContain(
      'le mot introuvable',
    );
  });

  it('upgrades on approach without waiting for idle time', () => {
    // A user who scrolls fast must not read raw markdown.
    enableDeferral();
    const { container } = render(<MarkdownContent content={MD} />);
    expect(observed).toHaveLength(1);

    act(() => trigger!([{ isIntersecting: true }]));

    expect(container.querySelector('strong')).not.toBeNull();
    // And the idle queue no longer has work for it.
    drainIdle();
    expect(container.querySelector('.disc-md-deferred')).toBeNull();
  });

  it('never goes back to text once rendered', () => {
    // Reverting would reflow the page under someone reading it.
    enableDeferral();
    const { container, rerender } = render(<MarkdownContent content={MD} />);
    drainIdle();

    rerender(<MarkdownContent content={MD} />);

    expect(container.querySelector('.disc-md-deferred')).toBeNull();
    expect(container.querySelector('strong')).not.toBeNull();
  });
});

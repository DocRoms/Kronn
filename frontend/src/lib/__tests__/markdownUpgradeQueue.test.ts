import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  queueMarkdownUpgrade,
  resetMarkdownUpgradeQueueForTests,
} from '../markdownUpgradeQueue';

/** Drives the queue the way the browser does: one idle slice with a budget. */
function runIdleSlice(msAvailable: number) {
  const callback = idleCallbacks.shift();
  if (!callback) throw new Error('nothing was scheduled');
  let left = msAvailable;
  callback({ timeRemaining: () => (left -= 1) + 1, didTimeout: false });
}

let idleCallbacks: ((deadline: { timeRemaining: () => number; didTimeout: boolean }) => void)[] = [];

describe('markdownUpgradeQueue', () => {
  beforeEach(() => {
    resetMarkdownUpgradeQueueForTests();
    idleCallbacks = [];
    vi.stubGlobal('requestIdleCallback', (cb: (d: never) => void) => {
      idleCallbacks.push(cb as never);
      return idleCallbacks.length;
    });
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('renders everything queued, not just the first slice', () => {
    // The whole point is that a room finishes rendering. A queue that stopped
    // after one slice would leave the top of the conversation as raw text.
    const done: number[] = [];
    for (let i = 0; i < 5; i += 1) queueMarkdownUpgrade(() => done.push(i));

    // A slice too small for all five: the rest must reschedule.
    runIdleSlice(3);
    expect(done.length).toBeGreaterThan(0);
    expect(done.length).toBeLessThan(5);

    while (idleCallbacks.length > 0) runIdleSlice(100);
    expect(done).toEqual([0, 1, 2, 3, 4]);
  });

  it('stops inside the slice instead of overrunning the frame', () => {
    const done: number[] = [];
    for (let i = 0; i < 50; i += 1) queueMarkdownUpgrade(() => done.push(i));

    runIdleSlice(10);

    expect(done.length).toBeLessThan(50);
    expect(idleCallbacks.length).toBe(1); // the remainder rescheduled itself
  });

  it('always makes progress, even when no time is offered', () => {
    // An idle callback can report zero remaining, and a busy tab reports it
    // precisely when there is most to render. Yielding without doing anything
    // reschedules forever: the room would stay raw text indefinitely.
    const done: number[] = [];
    for (let i = 0; i < 20; i += 1) queueMarkdownUpgrade(() => done.push(i));

    runIdleSlice(0);
    expect(done).toEqual([0]);

    // And it keeps draining one per starved slice rather than stalling.
    runIdleSlice(0);
    runIdleSlice(0);
    expect(done).toEqual([0, 1, 2]);
  });

  it('forgets an upgrade that was cancelled', () => {
    // A message unmounted while queued: running its setState would warn and
    // leak. The cancel handle is what the effect cleanup calls.
    const done: string[] = [];
    const cancel = queueMarkdownUpgrade(() => done.push('gone'));
    queueMarkdownUpgrade(() => done.push('kept'));

    cancel();
    while (idleCallbacks.length > 0) runIdleSlice(100);

    expect(done).toEqual(['kept']);
  });

  it('does not let one failure strand the messages behind it', () => {
    const done: string[] = [];
    queueMarkdownUpgrade(() => {
      throw new Error('render blew up');
    });
    queueMarkdownUpgrade(() => done.push('after'));

    while (idleCallbacks.length > 0) runIdleSlice(100);

    expect(done).toEqual(['after']);
  });
});

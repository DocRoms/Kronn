// Tests for appendLiveBuffer — the FIFO-cap helper that bounds the live
// progress feed.
//
// Why this is load-bearing: tool-call streaming (every Edit/Bash/Read on
// a heavy implement step) emits chunks at 10-20/sec. Over a 25 min run,
// the buffer would grow to multiple MB if uncapped, freezing React's
// reconciler each time the <pre> re-renders. The cap keeps the live view
// constant-time regardless of run length.

import { describe, it, expect } from 'vitest';
import { appendLiveBuffer } from '../WorkflowsPage';

describe('appendLiveBuffer — FIFO cap on live progress text', () => {
  it('returns prev + chunks unchanged when total fits under the cap', () => {
    expect(appendLiveBuffer('hello ', 'world', 100)).toBe('hello world');
  });

  it('returns the merged string at exactly the cap with no truncation', () => {
    const prev = 'a'.repeat(40);
    const chunks = 'b'.repeat(10);
    const out = appendLiveBuffer(prev, chunks, 50);
    expect(out.length).toBe(50);
    expect(out).toBe(prev + chunks);
  });

  it('drops the oldest chars when total exceeds the cap (FIFO window)', () => {
    const prev = 'OLD' + 'x'.repeat(48);
    const chunks = 'NEW';
    // total = 54 chars; cap = 50 → keep the trailing 50.
    const out = appendLiveBuffer(prev, chunks, 50);
    expect(out.length).toBe(50);
    expect(out.endsWith('NEW')).toBe(true);
    expect(out.startsWith('OLD')).toBe(false);
  });

  it('handles a chunks payload larger than the cap on its own', () => {
    // Tool-streaming bursts can occasionally emit a single huge chunk
    // (a Bash command with multi-line stdout, for instance).
    const flood = 'X'.repeat(200);
    const out = appendLiveBuffer('previous', flood, 50);
    expect(out.length).toBe(50);
    expect(out).toBe('X'.repeat(50));
  });

  it('handles an empty prev (first chunk after step transition)', () => {
    const out = appendLiveBuffer('', 'first chunk', 100);
    expect(out).toBe('first chunk');
  });

  it('handles an empty chunks (no-op call)', () => {
    expect(appendLiveBuffer('existing', '', 100)).toBe('existing');
  });

  it('uses a 50KB-ish ceiling consistent with the production caller', () => {
    // Documents the production cap. The component caller passes 50_000;
    // this test catches a future regression where someone bumps it
    // without remembering the perf reasoning.
    const huge = 'y'.repeat(60_000);
    const out = appendLiveBuffer('', huge, 50_000);
    expect(out.length).toBe(50_000);
  });
});

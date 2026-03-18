/**
 * Tests for discussions._streamSSE — the SSE streaming helper in api.ts.
 *
 * SSE wire format used by this implementation:
 *   event: chunk\n
 *   data: {"text":"hello"}\n
 *   \n
 *
 * The function reads lines, tracks the current event type, and dispatches to
 * onChunk / onDone / onError based on the event type field.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// ─── Helpers ─────────────────────────────────────────────────────────────────

function makeSSEStream(events: string[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const event of events) {
        controller.enqueue(encoder.encode(event));
      }
      controller.close();
    },
  });
}

/** Build a minimal fetch Response that streams the given SSE events. */
function mockStreamingFetch(events: string[], status = 200) {
  const body = makeSSEStream(events);
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    body,
    headers: { get: () => null },
  }));
}

/** Build a successful SSE chunk event line-pair. */
function sseChunk(text: string): string {
  return `event: chunk\ndata: ${JSON.stringify({ text })}\n\n`;
}

const sseDone = 'event: done\ndata: {}\n\n';

// ─── Setup / teardown ─────────────────────────────────────────────────────────

beforeEach(() => {
  // reset modules so _streamSSE always gets a fresh fetch stub
  vi.resetModules();
});

afterEach(() => {
  vi.restoreAllMocks();
});

// ─── Helpers to call _streamSSE ───────────────────────────────────────────────

async function callStreamSSE(
  events: string[],
  options: {
    status?: number;
    signal?: AbortSignal;
    onStart?: () => void;
  } = {},
) {
  const { status = 200, signal, onStart } = options;
  mockStreamingFetch(events, status);

  const { discussions } = await import('../api');
  const onChunk = vi.fn();
  const onDone = vi.fn();
  const onError = vi.fn();

  await discussions._streamSSE(
    '/api/discussions/test-id/messages',
    { content: 'hello' },
    onChunk,
    onDone,
    onError,
    signal,
    onStart,
  );

  return { onChunk, onDone, onError };
}

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('_streamSSE — normal streaming', () => {
  it('calls onChunk with the text from a single chunk event', async () => {
    const { onChunk, onDone, onError } = await callStreamSSE([
      sseChunk('hello'),
      sseDone,
    ]);

    expect(onChunk).toHaveBeenCalledOnce();
    expect(onChunk).toHaveBeenCalledWith('hello');
    expect(onDone).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
  });

  it('calls onDone exactly once even when done event appears multiple times', async () => {
    const { onDone } = await callStreamSSE([
      sseChunk('x'),
      sseDone,
      sseDone, // duplicate — should be ignored thanks to the `finished` guard
    ]);

    expect(onDone).toHaveBeenCalledOnce();
  });

  it('calls onChunk for each chunk event received', async () => {
    const { onChunk, onDone, onError } = await callStreamSSE([
      sseChunk('foo'),
      sseChunk('bar'),
      sseChunk('baz'),
      sseDone,
    ]);

    expect(onChunk).toHaveBeenCalledTimes(3);
    expect(onChunk).toHaveBeenNthCalledWith(1, 'foo');
    expect(onChunk).toHaveBeenNthCalledWith(2, 'bar');
    expect(onChunk).toHaveBeenNthCalledWith(3, 'baz');
    expect(onDone).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
  });
});

describe('_streamSSE — multiple chunks accumulation', () => {
  it('handles multiple SSE events delivered in a single stream read', async () => {
    // All events arrive in one read — the buffer logic splits them by newline
    // and dispatches each event type+data pair correctly.
    const { onChunk, onDone, onError } = await callStreamSSE([
      sseChunk('first') + sseChunk('second') + sseChunk('third') + sseDone,
    ]);

    expect(onChunk).toHaveBeenCalledTimes(3);
    expect(onChunk).toHaveBeenNthCalledWith(1, 'first');
    expect(onChunk).toHaveBeenNthCalledWith(2, 'second');
    expect(onChunk).toHaveBeenNthCalledWith(3, 'third');
    expect(onDone).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
  });

  it('handles partial last line buffered until next read', async () => {
    // First read ends mid-line (no trailing \n) — buffer should hold it
    // and the second read completes the event.
    const encoder = new TextEncoder();
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        // First read: full chunk event + start of done event (no trailing newline)
        controller.enqueue(encoder.encode('event: chunk\ndata: {"text":"buffered"}\n\nevent: done\ndata: {'));
        // Second read: closes the JSON + final newlines
        controller.enqueue(encoder.encode('}\n\n'));
        controller.close();
      },
    });

    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      body,
      headers: { get: () => null },
    }));

    vi.resetModules();
    const { discussions } = await import('../api');
    const onChunk = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();

    await discussions._streamSSE(
      '/api/discussions/x/messages',
      null,
      onChunk,
      onDone,
      onError,
    );

    expect(onChunk).toHaveBeenCalledWith('buffered');
    expect(onDone).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
  });
});

describe('_streamSSE — abort mid-stream', () => {
  it('resolves without calling onError when fetch is aborted before response', async () => {
    // Simulate an AbortError thrown by fetch
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(
      Object.assign(new DOMException('Aborted', 'AbortError'), { name: 'AbortError' }),
    ));

    vi.resetModules();
    const { discussions } = await import('../api');
    const onChunk = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();

    const controller = new AbortController();
    controller.abort();

    await discussions._streamSSE(
      '/api/discussions/x/messages',
      null,
      onChunk,
      onDone,
      onError,
      controller.signal,
    );

    // When aborted during fetch, onDone is called (stream considered complete)
    // and onError must NOT be called
    expect(onError).not.toHaveBeenCalled();
    expect(onChunk).not.toHaveBeenCalled();
  });

  it('resolves without calling onError when stream reader throws AbortError', async () => {
    // Simulate abort happening while reading the stream body
    const encoder = new TextEncoder();
    let callCount = 0;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(encoder.encode(sseChunk('partial')));
        // Do not close — reader.read() will block until aborted
      },
      pull(controller) {
        callCount++;
        if (callCount > 1) {
          // Throw AbortError to simulate signal-triggered cancellation
          controller.error(new DOMException('Aborted', 'AbortError'));
        }
      },
    });

    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      body,
      headers: { get: () => null },
    }));

    vi.resetModules();
    const { discussions } = await import('../api');
    const onChunk = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();

    await discussions._streamSSE(
      '/api/discussions/x/messages',
      null,
      onChunk,
      onDone,
      onError,
    );

    expect(onError).not.toHaveBeenCalled();
  });
});

describe('_streamSSE — server error (non-200 response)', () => {
  it('calls onError with HTTP status when response is not ok', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: false,
      status: 500,
      body: null,
      headers: { get: () => null },
    }));

    vi.resetModules();
    const { discussions } = await import('../api');
    const onChunk = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();

    await discussions._streamSSE(
      '/api/discussions/x/messages',
      null,
      onChunk,
      onDone,
      onError,
    );

    expect(onError).toHaveBeenCalledWith('HTTP 500');
    expect(onChunk).not.toHaveBeenCalled();
    expect(onDone).not.toHaveBeenCalled();
  });

  it('calls onError when body is null even if status is 200', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
      ok: true,
      status: 200,
      body: null,
      headers: { get: () => null },
    }));

    vi.resetModules();
    const { discussions } = await import('../api');
    const onChunk = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();

    await discussions._streamSSE(
      '/api/discussions/x/messages',
      null,
      onChunk,
      onDone,
      onError,
    );

    expect(onError).toHaveBeenCalledWith('HTTP 200');
  });
});

describe('_streamSSE — malformed SSE data', () => {
  it('ignores non-JSON data lines and continues processing', async () => {
    const { onChunk, onDone, onError } = await callStreamSSE([
      'event: chunk\ndata: NOT_VALID_JSON\n\n',
      sseChunk('valid'),
      sseDone,
    ]);

    // The invalid JSON line is silently ignored
    expect(onChunk).toHaveBeenCalledOnce();
    expect(onChunk).toHaveBeenCalledWith('valid');
    expect(onDone).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
  });

  it('ignores chunk events where text field is missing', async () => {
    const { onChunk, onDone, onError } = await callStreamSSE([
      'event: chunk\ndata: {"no_text_field":true}\n\n',
      sseChunk('ok'),
      sseDone,
    ]);

    // First chunk has no `text` field — onChunk should not be called for it
    expect(onChunk).toHaveBeenCalledOnce();
    expect(onChunk).toHaveBeenCalledWith('ok');
    expect(onDone).toHaveBeenCalledOnce();
    expect(onError).not.toHaveBeenCalled();
  });

  it('handles server-sent error event with error field', async () => {
    const { onChunk, onError } = await callStreamSSE([
      'event: error\ndata: {"error":"something went wrong"}\n\n',
    ]);

    expect(onError).toHaveBeenCalledWith('something went wrong');
    expect(onChunk).not.toHaveBeenCalled();
  });

  it('handles server-sent error event without error field (fallback message)', async () => {
    const { onChunk, onError } = await callStreamSSE([
      'event: error\ndata: {}\n\n',
    ]);

    expect(onError).toHaveBeenCalledWith('Unknown error');
    expect(onChunk).not.toHaveBeenCalled();
  });

  it('handles completely empty stream gracefully', async () => {
    const { onChunk, onDone, onError } = await callStreamSSE([]);

    // Stream ends immediately — onDone is called, nothing else
    expect(onChunk).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
    expect(onDone).toHaveBeenCalledOnce();
  });
});

describe('_streamSSE — onStart callback', () => {
  it('calls onStart after a successful fetch response is received', async () => {
    const onStart = vi.fn();
    const { onDone } = await callStreamSSE([sseDone], { onStart });

    expect(onStart).toHaveBeenCalledOnce();
    expect(onDone).toHaveBeenCalledOnce();
  });

  it('does not call onStart when fetch is aborted', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(
      Object.assign(new DOMException('Aborted', 'AbortError'), { name: 'AbortError' }),
    ));

    vi.resetModules();
    const { discussions } = await import('../api');
    const onStart = vi.fn();
    const onChunk = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();

    await discussions._streamSSE(
      '/api/discussions/x/messages',
      null,
      onChunk,
      onDone,
      onError,
      undefined,
      onStart,
    );

    expect(onStart).not.toHaveBeenCalled();
  });
});

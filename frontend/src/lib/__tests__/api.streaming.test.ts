/**
 * 2026-05-29 — SSE-streaming methods of api.ts that drive the live UIs
 * (audit progress, multi-agent debate, workflow run view). `_streamSSE`
 * (chat) is covered by `streaming.test.ts` and `fullAuditStream` by
 * `api.fullAuditStream.legacyDocs.test.ts`; this file pins the remaining
 * streamers:
 *   - projects.auditStream / partialAuditStream  (fetchAndParseSSE based)
 *   - discussions.orchestrate                    (inline getReader loop)
 *   - discussions.sendMessageStream / runAgent   (delegate to _streamSSE)
 *   - workflows.triggerStream                    (inline getReader loop)
 *   - workflows.testStepStream                   (parseSSEStream based)
 *
 * These are the paths where "client sees a spinner but the event never
 * fires" bugs hide — so we assert the full event→handler dispatch table,
 * the HTTP-error path, and the abort path on a representative subset.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

function makeSSEStream(events: string[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const event of events) controller.enqueue(encoder.encode(event));
      controller.close();
    },
  });
}

/** Mock fetch to stream the given raw SSE event strings. */
function mockStreamingFetch(events: string[], status = 200) {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    body: makeSSEStream(events),
    headers: { get: () => null },
  }));
}

/** Mock fetch that rejects with an AbortError (signal aborted before/at fetch). */
function mockAbortingFetch() {
  const err = Object.assign(new Error('aborted'), { name: 'AbortError' });
  vi.stubGlobal('fetch', vi.fn().mockRejectedValue(err));
}

function sse(event: string, data: unknown): string {
  return `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`;
}

beforeEach(() => { vi.resetModules(); });
afterEach(() => { vi.restoreAllMocks(); });

// ════════════════════════════════════════════════════════════════════════════
// projects.auditStream
// ════════════════════════════════════════════════════════════════════════════
describe('projects.auditStream', () => {
  it('dispatches step_start / chunk / step_done / done to the right handlers', async () => {
    mockStreamingFetch([
      sse('step_start', { step: 1, total: 10, file: 'docs/AGENTS.md' }),
      sse('chunk', { text: 'analysing', step: 1 }),
      sse('step_done', { step: 1, success: true }),
      sse('done', {}),
    ]);
    const { projects } = await import('../api');
    const h = {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(),
      onDone: vi.fn(), onError: vi.fn(),
    };
    await projects.auditStream('p-1', {} as never, h);
    expect(h.onStepStart).toHaveBeenCalledWith(1, 10, 'docs/AGENTS.md');
    expect(h.onChunk).toHaveBeenCalledWith('analysing', 1);
    expect(h.onStepDone).toHaveBeenCalledWith(1, true);
    expect(h.onDone).toHaveBeenCalledTimes(1);
    expect(h.onError).not.toHaveBeenCalled();
  });

  it('routes step_error and error events to onError', async () => {
    mockStreamingFetch([
      sse('step_error', { error: 'step 3 blew up' }),
      sse('error', { error: 'fatal' }),
    ]);
    const { projects } = await import('../api');
    const onError = vi.fn();
    await projects.auditStream('p-1', {} as never, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone: vi.fn(), onError,
    });
    expect(onError).toHaveBeenCalledWith('step 3 blew up');
    expect(onError).toHaveBeenCalledWith('fatal');
  });

  it('surfaces an HTTP error as onError("HTTP <status>")', async () => {
    mockStreamingFetch([], 500);
    const { projects } = await import('../api');
    const onError = vi.fn();
    await projects.auditStream('p-1', {} as never, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone: vi.fn(), onError,
    });
    expect(onError).toHaveBeenCalledWith('HTTP 500');
  });

  it('calls onDone exactly once even if a done event precedes stream close', async () => {
    mockStreamingFetch([sse('done', {}), sse('done', {})]);
    const { projects } = await import('../api');
    const onDone = vi.fn();
    await projects.auditStream('p-1', {} as never, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError: vi.fn(),
    });
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it('treats an aborted fetch as a clean done (no error)', async () => {
    mockAbortingFetch();
    const { projects } = await import('../api');
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.auditStream('p-1', {} as never, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError,
    });
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onError).not.toHaveBeenCalled();
  });
});

// ════════════════════════════════════════════════════════════════════════════
// projects.partialAuditStream (same dispatch table, different endpoint)
// ════════════════════════════════════════════════════════════════════════════
describe('projects.partialAuditStream', () => {
  it('POSTs to /partial-audit and dispatches events', async () => {
    mockStreamingFetch([
      sse('step_start', { step: 4, total: 4, file: 'x' }),
      sse('step_done', { step: 4, success: false }),
      sse('done', {}),
    ]);
    const { projects } = await import('../api');
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    const h = { onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone: vi.fn(), onError: vi.fn() };
    await projects.partialAuditStream('p-1', { agent: 'ClaudeCode', steps: [4] }, h);
    expect(fetchSpy.mock.calls[0][0]).toMatch(/\/api\/projects\/p-1\/partial-audit$/);
    expect(h.onStepStart).toHaveBeenCalledWith(4, 4, 'x');
    expect(h.onStepDone).toHaveBeenCalledWith(4, false);
    expect(h.onDone).toHaveBeenCalledTimes(1);
  });
});

// ════════════════════════════════════════════════════════════════════════════
// discussions.orchestrate (inline reader loop, richer event set)
// ════════════════════════════════════════════════════════════════════════════
describe('discussions.orchestrate', () => {
  it('dispatches system / round / agent_start / chunk / agent_done / done', async () => {
    mockStreamingFetch([
      sse('system', { text: 'debate starting' }),
      sse('round', { round: 1, total: 3 }),
      sse('agent_start', { agent: 'Alpha', agent_type: 'ClaudeCode', round: 1 }),
      sse('chunk', { text: 'my take', agent: 'Alpha', agent_type: 'ClaudeCode', round: 1 }),
      sse('agent_done', { agent: 'Alpha', agent_type: 'ClaudeCode', round: 1 }),
      sse('done', {}),
    ]);
    const { discussions } = await import('../api');
    const h = {
      onSystem: vi.fn(), onRound: vi.fn(), onAgentStart: vi.fn(),
      onChunk: vi.fn(), onAgentDone: vi.fn(), onDone: vi.fn(), onError: vi.fn(),
    };
    await discussions.orchestrate('d-1', {} as never, h);
    expect(h.onSystem).toHaveBeenCalledWith('debate starting');
    expect(h.onRound).toHaveBeenCalledWith(1, 3);
    expect(h.onAgentStart).toHaveBeenCalledWith('Alpha', 'ClaudeCode', 1);
    expect(h.onChunk).toHaveBeenCalledWith('my take', 'Alpha', 'ClaudeCode', 1);
    expect(h.onAgentDone).toHaveBeenCalledWith('Alpha', 'ClaudeCode', 1);
    expect(h.onDone).toHaveBeenCalledTimes(1);
  });

  it('surfaces an HTTP error', async () => {
    mockStreamingFetch([], 503);
    const { discussions } = await import('../api');
    const onError = vi.fn();
    await discussions.orchestrate('d-1', {} as never, {
      onSystem: vi.fn(), onRound: vi.fn(), onAgentStart: vi.fn(),
      onChunk: vi.fn(), onAgentDone: vi.fn(), onDone: vi.fn(), onError,
    });
    expect(onError).toHaveBeenCalledWith('HTTP 503');
  });

  it('treats an aborted fetch as a clean done', async () => {
    mockAbortingFetch();
    const { discussions } = await import('../api');
    const onDone = vi.fn();
    const onError = vi.fn();
    await discussions.orchestrate('d-1', {} as never, {
      onSystem: vi.fn(), onRound: vi.fn(), onAgentStart: vi.fn(),
      onChunk: vi.fn(), onAgentDone: vi.fn(), onDone, onError,
    });
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onError).not.toHaveBeenCalled();
  });
});

// ════════════════════════════════════════════════════════════════════════════
// discussions.sendMessageStream / runAgent — delegate to _streamSSE
// ════════════════════════════════════════════════════════════════════════════
describe('discussions.sendMessageStream / runAgent', () => {
  it('sendMessageStream streams chunks then done, POSTing to /messages', async () => {
    mockStreamingFetch([sse('chunk', { text: 'hi' }), sse('done', {})]);
    const { discussions } = await import('../api');
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    const onChunk = vi.fn(); const onDone = vi.fn(); const onError = vi.fn();
    await discussions.sendMessageStream('d-1', { content: 'hi' } as never, onChunk, onDone, onError);
    expect(fetchSpy.mock.calls[0][0]).toMatch(/\/api\/discussions\/d-1\/messages$/);
    expect(onChunk).toHaveBeenCalledWith('hi');
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onError).not.toHaveBeenCalled();
  });

  it('runAgent POSTs to /run with a null body', async () => {
    mockStreamingFetch([sse('chunk', { text: 'go' }), sse('done', {})]);
    const { discussions } = await import('../api');
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    const onChunk = vi.fn(); const onDone = vi.fn();
    await discussions.runAgent('d-1', onChunk, onDone, vi.fn());
    expect(fetchSpy.mock.calls[0][0]).toMatch(/\/api\/discussions\/d-1\/run$/);
    expect((fetchSpy.mock.calls[0][1] as { body?: string }).body).toBeUndefined();
    expect(onChunk).toHaveBeenCalledWith('go');
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it('forwards log events to onLog', async () => {
    mockStreamingFetch([sse('log', { text: '🔧 ran a tool' }), sse('done', {})]);
    const { discussions } = await import('../api');
    const onLog = vi.fn();
    await discussions.sendMessageStream('d-1', { content: 'x' } as never, vi.fn(), vi.fn(), vi.fn(), undefined, undefined, onLog);
    expect(onLog).toHaveBeenCalledWith('🔧 ran a tool');
  });
});

// ════════════════════════════════════════════════════════════════════════════
// workflows.triggerStream (inline reader loop + run_start/step_progress)
// ════════════════════════════════════════════════════════════════════════════
describe('workflows.triggerStream', () => {
  it('dispatches run_start / step_start / step_progress / step_done / run_done', async () => {
    mockStreamingFetch([
      sse('run_start', { run_id: 'run-42' }),
      sse('step_start', { step_name: 'build', step_index: 0, total_steps: 2 }),
      sse('step_progress', { text: 'compiling' }),
      sse('step_done', { name: 'build', status: 'Ok' }),
      sse('run_done', { status: 'Completed' }),
    ]);
    const { workflows } = await import('../api');
    const onStepStart = vi.fn(); const onStepDone = vi.fn(); const onRunDone = vi.fn();
    const onError = vi.fn(); const onStepProgress = vi.fn(); const onRunStart = vi.fn();
    await workflows.triggerStream('wf-1', onStepStart, onStepDone, onRunDone, onError, undefined, undefined, onStepProgress, onRunStart);
    expect(onRunStart).toHaveBeenCalledWith('run-42');
    expect(onStepStart).toHaveBeenCalledWith({ step_name: 'build', step_index: 0, total_steps: 2 });
    expect(onStepProgress).toHaveBeenCalledWith('compiling');
    expect(onStepDone).toHaveBeenCalledWith({ name: 'build', status: 'Ok' });
    expect(onRunDone).toHaveBeenCalledWith({ status: 'Completed' });
    expect(onError).not.toHaveBeenCalled();
  });

  it('sends variables as a JSON body when provided', async () => {
    mockStreamingFetch([sse('run_done', { status: 'Completed' })]);
    const { workflows } = await import('../api');
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    await workflows.triggerStream('wf-1', vi.fn(), vi.fn(), vi.fn(), vi.fn(), undefined, { env: 'prod' });
    const opts = fetchSpy.mock.calls[0][1] as { headers: Record<string, string>; body: string };
    expect(opts.headers['Content-Type']).toBe('application/json');
    expect(JSON.parse(opts.body)).toEqual({ variables: { env: 'prod' } });
  });

  it('omits the body when variables is empty', async () => {
    mockStreamingFetch([sse('run_done', { status: 'Completed' })]);
    const { workflows } = await import('../api');
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    await workflows.triggerStream('wf-1', vi.fn(), vi.fn(), vi.fn(), vi.fn(), undefined, {});
    expect((fetchSpy.mock.calls[0][1] as { body?: string }).body).toBeUndefined();
  });

  it('surfaces an HTTP error', async () => {
    mockStreamingFetch([], 500);
    const { workflows } = await import('../api');
    const onError = vi.fn();
    await workflows.triggerStream('wf-1', vi.fn(), vi.fn(), vi.fn(), onError);
    expect(onError).toHaveBeenCalledWith('HTTP 500');
  });
});

// ════════════════════════════════════════════════════════════════════════════
// workflows.testStepStream (parseSSEStream based, dry-run)
// ════════════════════════════════════════════════════════════════════════════
describe('workflows.testStepStream', () => {
  it('POSTs to /workflows/test-step and dispatches the dry-run events', async () => {
    mockStreamingFetch([
      sse('step_start', { step_name: 's', step_index: 0, total_steps: 1 }),
      sse('step_progress', { text: 'half' }),
      sse('step_done', { name: 's', status: 'Ok' }),
      sse('run_done', { status: 'Completed' }),
    ]);
    const { workflows } = await import('../api');
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>;
    const onStepStart = vi.fn(); const onStepDone = vi.fn(); const onRunDone = vi.fn();
    const onError = vi.fn(); const onProgress = vi.fn();
    await workflows.testStepStream({} as never, onStepStart, onStepDone, onRunDone, onError, undefined, onProgress);
    expect(fetchSpy.mock.calls[0][0]).toMatch(/\/api\/workflows\/test-step$/);
    expect(onStepStart).toHaveBeenCalledWith({ step_name: 's', step_index: 0, total_steps: 1 });
    expect(onProgress).toHaveBeenCalledWith('half');
    expect(onStepDone).toHaveBeenCalledWith({ name: 's', status: 'Ok' });
    expect(onRunDone).toHaveBeenCalledWith({ status: 'Completed' });
  });

  it('surfaces an HTTP error', async () => {
    mockStreamingFetch([], 422);
    const { workflows } = await import('../api');
    const onError = vi.fn();
    await workflows.testStepStream({} as never, vi.fn(), vi.fn(), vi.fn(), onError);
    expect(onError).toHaveBeenCalledWith('HTTP 422');
  });
});

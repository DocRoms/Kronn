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
// projects.partialAuditStream — carries the generic SSE dispatch coverage
// (HTTP error, double-done, abort-as-done) since the legacy auditStream /
// POST /ai-audit pair was removed with its lease-bypassing backend route.
// ════════════════════════════════════════════════════════════════════════════
describe('projects.partialAuditStream', () => {
  const REQ = { agent: 'ClaudeCode', steps: [1] } as never;

  it('step_error is NON-terminal (onStepError), event:error is terminal (onError, onDone sealed)', async () => {
    mockStreamingFetch([
      sse('step_error', { error: 'step 3 blew up', step: 3 }),
      sse('error', { error: 'fatal' }),
    ]);
    const { projects } = await import('../api');
    const onError = vi.fn();
    const onStepError = vi.fn();
    const onDone = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onStepError, onDone, onError,
    });
    expect(onStepError).toHaveBeenCalledWith('step 3 blew up', 3);
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith('fatal');
    expect(onDone).not.toHaveBeenCalled();
  });

  it('surfaces an HTTP error as a single terminal onError — onDone never fires', async () => {
    mockStreamingFetch([], 500);
    const { projects } = await import('../api');
    const onError = vi.fn();
    const onDone = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError,
    });
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith('HTTP 500');
    expect(onDone).not.toHaveBeenCalled();
  });

  // Satisfies every matrix-v2 invariant for REQ (steps: [1]).
  const START_OK = { total_steps: 1, requested_steps: [1] };
  const DONE_OK = { status: 'complete', succeeded_steps: [1], unchanged_steps: [], failed_steps: [],
                    discussion_id: 'd-ok', audit_run_id: 'r-ok' };

  it('calls onDone exactly once even if a done event precedes stream close', async () => {
    mockStreamingFetch([sse('start', START_OK), sse('done', DONE_OK), sse('done', DONE_OK)]);
    const { projects } = await import('../api');
    const onDone = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError: vi.fn(),
    });
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it('treats an aborted fetch as a clean done (no error)', async () => {
    mockAbortingFetch();
    const { projects } = await import('../api');
    const onDone = vi.fn();
    const onError = vi.fn();
    const controller = new AbortController();
    controller.abort();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError,
    }, controller.signal);
    expect(onDone).toHaveBeenCalledTimes(1);
    expect(onDone).toHaveBeenCalledWith(undefined);
    expect(onError).not.toHaveBeenCalled();
  });

  it('an EOF without any terminal event is a terminal failure, never a clean done', async () => {
    // A backend crash / proxy cut after start must not bypass the terminal
    // seal by simply closing the socket (the MCP bridge distinguishes
    // stream_closed the same way).
    mockStreamingFetch([sse('start', START_OK)]);
    const { projects } = await import('../api');
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError,
    });
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith(expect.stringContaining('closed before a terminal event'));
    expect(onDone).not.toHaveBeenCalled();
  });

  it('dispatches validation_created and the done payload (A5 scoped validation)', async () => {
    mockStreamingFetch([
      sse('start', { total_steps: 1, requested_steps: [1] }),
      sse('step_start', { step: 3, progress: 1, total: 1, file: 'x' }),
      sse('step_done', { step: 3, success: true }),
      sse('validation_created', { discussion_id: 'd-scoped' }),
      sse('done', { status: 'complete', discussion_id: 'd-scoped', audit_run_id: 'run-9',
                    succeeded_steps: [1], unchanged_steps: [], failed_steps: [] }),
    ]);
    const { projects } = await import('../api');
    const onValidationCreated = vi.fn();
    const onDone = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(),
      onValidationCreated, onDone, onError: vi.fn(),
    });
    expect(onValidationCreated).toHaveBeenCalledWith('d-scoped');
    expect(onDone).toHaveBeenCalledWith({
      status: 'complete', discussionId: 'd-scoped', auditRunId: 'run-9',
      succeededSteps: [1], unchangedSteps: [], failedSteps: [],
    });
  });

  it('an interrupted done carries its status, null ids and the partition', async () => {
    mockStreamingFetch([
      sse('start', { total_steps: 2, requested_steps: [3, 8] }),
      sse('step_unchanged', { step: 3, file: 'docs/repo-map.md' }),
      sse('done', { status: 'interrupted', succeeded_steps: [], unchanged_steps: [3], failed_steps: [8],
                    audit_run_id: 'r-int' }),
    ]);
    const { projects } = await import('../api');
    const onDone = vi.fn();
    const onStepUnchanged = vi.fn();
    await projects.partialAuditStream('p-1', { agent: 'ClaudeCode', steps: [3, 8] } as never, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onStepUnchanged, onDone, onError: vi.fn(),
    });
    expect(onStepUnchanged).toHaveBeenCalledWith(3, 'docs/repo-map.md');
    expect(onDone).toHaveBeenCalledWith({
      status: 'interrupted', discussionId: null, auditRunId: 'r-int',
      succeededSteps: [], unchangedSteps: [3], failedSteps: [8],
    });
  });

  it('POSTs to /partial-audit and dispatches events', async () => {
    mockStreamingFetch([
      sse('start', { total_steps: 1, requested_steps: [4] }),
      sse('step_start', { step: 4, total: 4, file: 'x' }),
      sse('step_done', { step: 4, success: false }),
      sse('done', { status: 'interrupted', succeeded_steps: [], unchanged_steps: [], failed_steps: [4],
                    audit_run_id: 'r-4' }),
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

  /** A refused done payload: onError exactly once, onDone NEVER — the error
   *  callback owns the closure, a second callback would double the toasts. */
  async function expectRefused(payload: unknown, errorMatch: string) {
    mockStreamingFetch([sse('start', START_OK), sse('done', payload)]);
    const { projects } = await import('../api');
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError,
    });
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith(expect.stringContaining(errorMatch));
    expect(onDone).not.toHaveBeenCalled();
  }

  it('an empty done payload is refused — onError once, onDone never', async () => {
    await expectRefused({}, 'Malformed audit done event');
  });

  it('an unknown done status is refused, not coerced to complete', async () => {
    await expectRefused(
      { status: 'accepted', succeeded_steps: [1], unchanged_steps: [], failed_steps: [] },
      'Malformed audit done event',
    );
  });

  it('a malformed step list is refused even with a valid status', async () => {
    await expectRefused(
      { status: 'complete', succeeded_steps: 'all', unchanged_steps: [], failed_steps: [] },
      'succeededSteps',
    );
  });

  it('a done without audit_run_id is refused whatever the status', async () => {
    await expectRefused(
      { status: 'interrupted', succeeded_steps: [], unchanged_steps: [], failed_steps: [1] },
      'missing audit_run_id',
    );
    await expectRefused(
      { status: 'no_change', succeeded_steps: [], unchanged_steps: [1], failed_steps: [] },
      'missing audit_run_id',
    );
  });

  it('overlapping step lists are refused', async () => {
    await expectRefused(
      { status: 'complete', succeeded_steps: [1], unchanged_steps: [1], failed_steps: [],
        discussion_id: 'd', audit_run_id: 'r' },
      'overlap',
    );
  });

  it('a partition that misses a requested step is refused', async () => {
    await expectRefused(
      { status: 'no_change', succeeded_steps: [], unchanged_steps: [], failed_steps: [] },
      'partition',
    );
  });

  it('complete without a validation discussion is refused, never a silent success', async () => {
    await expectRefused(
      { status: 'complete', succeeded_steps: [1], unchanged_steps: [], failed_steps: [],
        audit_run_id: 'r-1' },
      'validation discussion',
    );
  });

  it('interrupted carrying a discussion is refused', async () => {
    await expectRefused(
      { status: 'interrupted', succeeded_steps: [], unchanged_steps: [], failed_steps: [1],
        discussion_id: 'd-forged', audit_run_id: 'r-1' },
      'cannot carry a discussion',
    );
  });

  it('a baseline reorder is legitimate: the done partition follows the CANONICAL start list', async () => {
    // The request names pre-reorder step 3; the backend re-routes it (by
    // ai_file) to slot 4 and says so in `start.requested_steps` — the done
    // partition [4] must be accepted even though req.steps was [3].
    mockStreamingFetch([
      sse('start', { total_steps: 1, requested_steps: [4] }),
      sse('done', { status: 'complete', succeeded_steps: [4], unchanged_steps: [], failed_steps: [],
                    discussion_id: 'd-reroute', audit_run_id: 'r-reroute' }),
    ]);
    const { projects } = await import('../api');
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', { agent: 'ClaudeCode', steps: [3] } as never, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError,
    });
    expect(onError).not.toHaveBeenCalled();
    expect(onDone).toHaveBeenCalledWith(expect.objectContaining({
      status: 'complete', succeededSteps: [4],
    }));
  });

  it('after a reorder, a done partition over the ORIGINAL request numbers is refused', async () => {
    mockStreamingFetch([
      sse('start', { total_steps: 1, requested_steps: [4] }),
      sse('done', { status: 'complete', succeeded_steps: [3], unchanged_steps: [], failed_steps: [],
                    discussion_id: 'd', audit_run_id: 'r' }),
    ]);
    const { projects } = await import('../api');
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', { agent: 'ClaudeCode', steps: [3] } as never, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError,
    });
    expect(onError).toHaveBeenCalledWith(expect.stringContaining('partition'));
    expect(onDone).not.toHaveBeenCalled();
  });

  it('a done before any start is refused — no canonical list to validate against', async () => {
    mockStreamingFetch([sse('done', DONE_OK)]);
    const { projects } = await import('../api');
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onDone, onError,
    });
    expect(onError).toHaveBeenCalledWith(expect.stringContaining('done before start'));
    expect(onDone).not.toHaveBeenCalled();
  });

  it('a mid-run spawn failure flows step_error → step_done → done interrupted, no terminal onError', async () => {
    mockStreamingFetch([
      sse('start', START_OK),
      sse('step_error', { error: 'spawn failed', step: 1 }),
      sse('step_done', { step: 1, success: false, outcome: 'failed', file: 'docs/x.md' }),
      sse('done', { status: 'interrupted', succeeded_steps: [], unchanged_steps: [], failed_steps: [1],
                    audit_run_id: 'r-1' }),
    ]);
    const { projects } = await import('../api');
    const onStepError = vi.fn();
    const onStepDone = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone, onStepError, onDone, onError,
    });
    expect(onStepError).toHaveBeenCalledWith('spawn failed', 1);
    expect(onStepDone).toHaveBeenCalledWith(1, false);
    expect(onDone).toHaveBeenCalledWith(expect.objectContaining({ status: 'interrupted' }));
    expect(onError).not.toHaveBeenCalled();
  });

  it('a mid-run stamp warning is non-terminal: the step still closes and done stays complete', async () => {
    mockStreamingFetch([
      sse('start', START_OK),
      sse('warning', { message: 'Step 1 (docs/x.md): audit-date stamp failed: disk full' }),
      sse('step_done', { step: 1, success: true, outcome: 'succeeded', file: 'docs/x.md' }),
      sse('done', DONE_OK),
    ]);
    const { projects } = await import('../api');
    const onWarning = vi.fn();
    const onStepDone = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone, onWarning, onDone, onError,
    });
    expect(onWarning).toHaveBeenCalledWith(expect.stringContaining('stamp failed'));
    expect(onStepDone).toHaveBeenCalledWith(1, true);
    expect(onDone).toHaveBeenCalledWith(expect.objectContaining({ status: 'complete' }));
    expect(onError).not.toHaveBeenCalled();
  });

  it('a post-commit baseline warning is non-terminal: onWarning then done complete', async () => {
    mockStreamingFetch([
      sse('start', START_OK),
      sse('warning', { message: 'Baseline write failed: disk full' }),
      sse('done', DONE_OK),
    ]);
    const { projects } = await import('../api');
    const onWarning = vi.fn();
    const onDone = vi.fn();
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', REQ, {
      onStepStart: vi.fn(), onChunk: vi.fn(), onStepDone: vi.fn(), onWarning, onDone, onError,
    });
    expect(onWarning).toHaveBeenCalledWith(expect.stringContaining('Baseline write failed'));
    expect(onDone).toHaveBeenCalledWith(expect.objectContaining({ status: 'complete' }));
    expect(onError).not.toHaveBeenCalled();
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

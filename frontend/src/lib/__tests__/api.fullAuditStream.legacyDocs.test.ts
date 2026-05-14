// 0.8.3 (#272) — focused SSE-dispatch test for the legacy-docs
// migration event emitted by `POST /api/projects/:id/full-audit`.
//
// The audit pipeline moves user-curated `docs/` content to
// `docs/legacy/` BEFORE installing Kronn templates, then surfaces a
// `legacy_docs_migrated` SSE event so the frontend can render a
// toast + list of moved entries. This test stubs `fetch` with a fake
// ReadableStream and verifies the dispatch wiring:
//   1. The new `onLegacyDocsMigrated` handler fires when the event
//      lands.
//   2. The payload reaches the handler verbatim (count + entries +
//      skip_reason).
//   3. Missing handler doesn't crash — older callers can opt out by
//      not passing it.
//
// Data-safety matters here: a regression that silently drops the
// event would mean the user thinks Kronn ate their files (no toast,
// no surfaced list) when in fact they're alive under `docs/legacy/`.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

/**
 * Build a fake `Response` whose body is a ReadableStream emitting the
 * given SSE chunks one after the other. Each entry is an
 * `event: <type>\ndata: <json>\n\n` block — the wire format the
 * backend uses.
 */
function fakeSseResponse(chunks: { event: string; data: object }[]): Response {
  const enc = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) {
        const wire = `event: ${c.event}\ndata: ${JSON.stringify(c.data)}\n\n`;
        controller.enqueue(enc.encode(wire));
      }
      controller.close();
    },
  });
  return new Response(body, {
    status: 200,
    headers: { 'content-type': 'text/event-stream' },
  });
}

describe('fullAuditStream — legacy_docs_migrated event (0.8.3 #272)', () => {
  it('fires onLegacyDocsMigrated with the full report payload', async () => {
    const report = {
      migrated: true,
      skip_reason: '',
      moved_entries: ['installation.md', 'api.md', 'architecture/overview.md'],
      moved_count: 3,
    };
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'legacy_docs_migrated', data: report },
        { event: 'template_installed', data: { installed: true } },
        { event: 'done', data: { discussion_id: 'd-1', template_was_installed: true } },
      ])
    );

    const { projects } = await import('../api');
    const onLegacyDocsMigrated = vi.fn();
    const onDone = vi.fn();
    const onTemplateInstalled = vi.fn();

    await projects.fullAuditStream(
      'p-1',
      { agent: 'ClaudeCode' },
      {
        onTemplateInstalled,
        onLegacyDocsMigrated,
        onStepStart: () => {},
        onChunk: () => {},
        onStepDone: () => {},
        onValidationCreated: () => {},
        onDone,
        onError: () => {},
      },
    );

    expect(onLegacyDocsMigrated).toHaveBeenCalledTimes(1);
    expect(onLegacyDocsMigrated).toHaveBeenCalledWith({
      migrated: true,
      skip_reason: '',
      moved_entries: ['installation.md', 'api.md', 'architecture/overview.md'],
      moved_count: 3,
    });
    // template_installed + done are also wired so the rest of the
    // pipeline isn't broken by the new event.
    expect(onTemplateInstalled).toHaveBeenCalledWith(true);
    expect(onDone).toHaveBeenCalledWith('d-1', true);
  });

  it('omitting onLegacyDocsMigrated does not crash older callers', async () => {
    // Backwards compat — the handler is optional. A page that hasn't
    // upgraded yet (no onLegacyDocsMigrated in its handler object)
    // must still receive the rest of the stream without throwing.
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'legacy_docs_migrated', data: { migrated: true, skip_reason: '', moved_entries: ['x.md'], moved_count: 1 } },
        { event: 'done', data: { discussion_id: 'd-2', template_was_installed: false } },
      ])
    );

    const { projects } = await import('../api');
    const onDone = vi.fn();
    await projects.fullAuditStream(
      'p-1',
      { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        // onLegacyDocsMigrated INTENTIONALLY OMITTED
        onStepStart: () => {},
        onChunk: () => {},
        onStepDone: () => {},
        onValidationCreated: () => {},
        onDone,
        onError: () => {},
      },
    );
    expect(onDone).toHaveBeenCalledWith('d-2', false);
  });

  // 0.8.3 (#274) — enriched audit progress events.
  // `start` carries an ISO-8601 `started_at` + `total_steps`; the
  // frontend uses these to anchor a live elapsed counter without
  // local-clock drift. `step_done` carries `tokens` (per-step) +
  // `total_tokens` (running sum) so the operator sees which step
  // burns the most and how much they've spent so far.

  it('start event forwards totalSteps + startedAt to onAuditStart', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'start', data: { total_steps: 10, started_at: '2026-05-14T17:00:00Z' } },
        { event: 'done', data: {} },
      ])
    );
    const { projects } = await import('../api');
    const onAuditStart = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        onAuditStart,
        onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {},
        onValidationCreated: () => {}, onDone: () => {}, onError: () => {},
      },
    );
    expect(onAuditStart).toHaveBeenCalledWith(10, '2026-05-14T17:00:00Z');
  });

  it('step_done forwards tokens + duration_ms + total_tokens positionally', async () => {
    // The enriched signature is `(step, success, tokens, durationMs,
    // totalTokens)`. Existing single/double-arg callers keep working
    // because the extra args are positional + optional.
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'step_done', data: {
            step: 3, success: true, file: 'docs/glossary.md',
            tokens: 4521, duration_ms: 18000, total_tokens: 12340,
          }
        },
        { event: 'done', data: {} },
      ])
    );
    const { projects } = await import('../api');
    const onStepDone = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        onStepStart: () => {}, onChunk: () => {}, onStepDone,
        onValidationCreated: () => {}, onDone: () => {}, onError: () => {},
      },
    );
    expect(onStepDone).toHaveBeenCalledWith(3, true, 4521, 18000, 12340);
  });

  it('step_done without tokens fields stays backwards-compatible', async () => {
    // Old backends (or non-stream-json agents like Vibe/Ollama) emit
    // step_done without `tokens` / `total_tokens`. The handler must
    // still fire with the legacy positional args and undefined for
    // the new ones — no crash, no broken chips.
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'step_done', data: { step: 1, success: true, file: 'docs/AGENTS.md' } },
        { event: 'done', data: {} },
      ])
    );
    const { projects } = await import('../api');
    const onStepDone = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        onStepStart: () => {}, onChunk: () => {}, onStepDone,
        onValidationCreated: () => {}, onDone: () => {}, onError: () => {},
      },
    );
    expect(onStepDone).toHaveBeenCalledWith(1, true, undefined, undefined, undefined);
  });

  // 0.8.3 (#281) — live in-step events: `step_progress` for token
  // counter ticking DURING a step, `tool_call` for the chip showing
  // which tool the agent just started invoking. Both replace the
  // pre-#281 silence where the user saw a spinner for 30-120s per
  // step with no signal of what's happening.

  it('step_progress event forwards (step, stepTokens, totalTokensSoFar) to onStepProgress', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'step_progress', data: { step: 2, step_tokens: 4521, total_tokens_so_far: 23890 } },
        { event: 'done', data: {} },
      ])
    );
    const { projects } = await import('../api');
    const onStepProgress = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        onStepProgress,
        onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {},
        onValidationCreated: () => {}, onDone: () => {}, onError: () => {},
      },
    );
    expect(onStepProgress).toHaveBeenCalledWith(2, 4521, 23890);
  });

  it('step_progress with missing total_tokens_so_far defaults to 0', async () => {
    // Defensive: if a future backend version omits the cumulative
    // field, the handler must still receive a valid number rather
    // than `undefined` (which would break arithmetic downstream).
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'step_progress', data: { step: 1, step_tokens: 100 } },
        { event: 'done', data: {} },
      ])
    );
    const { projects } = await import('../api');
    const onStepProgress = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        onStepProgress,
        onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {},
        onValidationCreated: () => {}, onDone: () => {}, onError: () => {},
      },
    );
    expect(onStepProgress).toHaveBeenCalledWith(1, 100, 0);
  });

  it('step_progress with non-numeric fields is silently ignored', async () => {
    // Type guard at the dispatch level: a malformed payload
    // shouldn't crash the SSE loop. Handler stays uncalled, the
    // stream continues to consume the next event.
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'step_progress', data: { step: 'not-a-number', step_tokens: 'x' } },
        { event: 'done', data: { discussion_id: 'd-ok' } },
      ])
    );
    const { projects } = await import('../api');
    const onStepProgress = vi.fn();
    const onDone = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        onStepProgress,
        onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {},
        onValidationCreated: () => {}, onDone, onError: () => {},
      },
    );
    expect(onStepProgress).not.toHaveBeenCalled();
    // The rest of the stream still flushed.
    expect(onDone).toHaveBeenCalledWith('d-ok', false);
  });

  it('tool_call event forwards (step, tool) to onToolCall', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'tool_call', data: { step: 3, tool: 'Read' } },
        { event: 'tool_call', data: { step: 3, tool: 'mcp__Sequential Thinking__sequentialthinking' } },
        { event: 'done', data: {} },
      ])
    );
    const { projects } = await import('../api');
    const onToolCall = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        onToolCall,
        onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {},
        onValidationCreated: () => {}, onDone: () => {}, onError: () => {},
      },
    );
    // Both calls reach the handler in order (last-write-wins is
    // the frontend's choice; SSE dispatch is faithful).
    expect(onToolCall).toHaveBeenNthCalledWith(1, 3, 'Read');
    expect(onToolCall).toHaveBeenNthCalledWith(2, 3, 'mcp__Sequential Thinking__sequentialthinking');
  });

  it('onStepProgress + onToolCall are optional — older callers compile', async () => {
    // Backwards compat sanity. Handlers absent → events flow
    // through without error.
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'step_progress', data: { step: 1, step_tokens: 100, total_tokens_so_far: 200 } },
        { event: 'tool_call', data: { step: 1, tool: 'Read' } },
        { event: 'done', data: { discussion_id: 'd-z' } },
      ])
    );
    const { projects } = await import('../api');
    const onDone = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        // No onStepProgress / onToolCall on purpose.
        onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {},
        onValidationCreated: () => {}, onDone, onError: () => {},
      },
    );
    expect(onDone).toHaveBeenCalledWith('d-z', false);
  });

  it('onAuditStart handler is optional — older callers compile + run', async () => {
    // No onAuditStart in the handler object → start event must be
    // silently ignored, the rest of the stream still processes.
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        { event: 'start', data: { total_steps: 10, started_at: '2026-05-14T17:00:00Z' } },
        { event: 'done', data: { discussion_id: 'd-x' } },
      ])
    );
    const { projects } = await import('../api');
    const onDone = vi.fn();
    await projects.fullAuditStream(
      'p-1', { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        // onAuditStart INTENTIONALLY OMITTED
        onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {},
        onValidationCreated: () => {}, onDone, onError: () => {},
      },
    );
    expect(onDone).toHaveBeenCalledWith('d-x', false);
  });

  it('fills missing fields with safe defaults when payload is partial', async () => {
    // Defensive: if a future backend version emits the event without
    // some fields (or the JSON gets truncated), the handler should
    // still receive a well-formed report rather than `undefined`.
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(
      fakeSseResponse([
        // Payload missing `moved_entries` AND `moved_count`.
        { event: 'legacy_docs_migrated', data: { migrated: true } },
        { event: 'done', data: {} },
      ])
    );

    const { projects } = await import('../api');
    const onLegacyDocsMigrated = vi.fn();
    await projects.fullAuditStream(
      'p-1',
      { agent: 'ClaudeCode' },
      {
        onTemplateInstalled: () => {},
        onLegacyDocsMigrated,
        onStepStart: () => {},
        onChunk: () => {},
        onStepDone: () => {},
        onValidationCreated: () => {},
        onDone: () => {},
        onError: () => {},
      },
    );

    expect(onLegacyDocsMigrated).toHaveBeenCalledWith({
      migrated: true,
      skip_reason: '',
      moved_entries: [],
      moved_count: 0,
    });
  });
});

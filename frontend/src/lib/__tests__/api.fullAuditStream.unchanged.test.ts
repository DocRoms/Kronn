// KT-1130 — the Full stream hands an `unchanged` step to its own handler.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

beforeEach(() => { vi.stubGlobal('fetch', vi.fn()); });
afterEach(() => { vi.restoreAllMocks(); });

function fakeSseResponse(chunks: { event: string; data: object }[]): Response {
  const enc = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      for (const c of chunks) controller.enqueue(enc.encode(`event: ${c.event}\ndata: ${JSON.stringify(c.data)}\n\n`));
      controller.close();
    },
  });
  return new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
}

describe('fullAuditStream — step_unchanged', () => {
  it('fires onStepUnchanged, never onStepWarning, and the step still closes', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(fakeSseResponse([
      { event: 'step_unchanged', data: { step: 4, file: 'docs/coding-rules.md' } },
      { event: 'step_done', data: { step: 4, success: true, outcome: 'unchanged', file: 'docs/coding-rules.md' } },
      { event: 'done', data: { status: 'complete', discussion_id: 'd-1', template_was_installed: false } },
    ]));
    const { projects } = await import('../api');
    const onStepUnchanged = vi.fn();
    const onStepWarning = vi.fn();
    const onStepDone = vi.fn();
    const onDone = vi.fn();
    await projects.fullAuditStream('p-1', { agent: 'ClaudeCode' }, {
      onTemplateInstalled: () => {}, onStepStart: () => {}, onChunk: () => {}, onStepDone,
      onStepUnchanged, onStepWarning, onValidationCreated: () => {}, onDone, onError: () => {},
    });
    expect(onStepUnchanged).toHaveBeenCalledWith(4, 'docs/coding-rules.md');
    expect(onStepWarning).not.toHaveBeenCalled();
    expect(onStepDone).toHaveBeenCalledWith(4, true, undefined, undefined, undefined);
    expect(onDone).toHaveBeenCalledWith('d-1', false, 'complete');
  });
});

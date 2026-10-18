// KT-1324 — a launch refused because of an agent a previous audit run left
// carries a typed `error_code` on its SSE `error` event; the full, partial and
// revalidation streams hand it to `onError` next to the text.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

beforeEach(() => { vi.stubGlobal('fetch', vi.fn()); });
afterEach(() => { vi.restoreAllMocks(); });

function errorResponse(data: object): Response {
  const enc = new TextEncoder();
  const body = new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(enc.encode(`event: error\ndata: ${JSON.stringify(data)}\n\n`));
      controller.close();
    },
  });
  return new Response(body, { status: 200, headers: { 'content-type': 'text/event-stream' } });
}

const refusal = { error: 'Kronn could not read the process table.', error_code: 'agents_unverified' };

describe('audit streams — error_code of a refused launch', () => {
  it('fullAuditStream passes it to onError', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(errorResponse(refusal));
    const { projects } = await import('../api');
    const onError = vi.fn();
    await projects.fullAuditStream('p-1', { agent: 'ClaudeCode' }, {
      onTemplateInstalled: () => {}, onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {},
      onValidationCreated: () => {}, onDone: () => {}, onError,
    });
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith(refusal.error, 'agents_unverified');
  });

  it('partialAuditStream passes it to onError', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(errorResponse(refusal));
    const { projects } = await import('../api');
    const onError = vi.fn();
    await projects.partialAuditStream('p-1', { agent: 'ClaudeCode', steps: [3] }, {
      onStepStart: () => {}, onChunk: () => {}, onStepDone: () => {}, onDone: () => {}, onError,
    });
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith(refusal.error, 'agents_unverified');
  });

  it('revalidateAuditStream passes it to onError', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(errorResponse(refusal));
    const { projects } = await import('../api');
    const onError = vi.fn();
    await projects.revalidateAuditStream('p-1', { agent: 'ClaudeCode' }, {
      onCancelled: () => {}, onDone: () => {}, onError,
    });
    expect(onError).toHaveBeenCalledTimes(1);
    expect(onError).toHaveBeenCalledWith(refusal.error, 'agents_unverified');
  });

  it('an error event without a code reaches onError with no code', async () => {
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValue(errorResponse({ error: 'agent crashed' }));
    const { projects } = await import('../api');
    const onError = vi.fn();
    await projects.revalidateAuditStream('p-1', { agent: 'ClaudeCode' }, {
      onCancelled: () => {}, onDone: () => {}, onError,
    });
    expect(onError).toHaveBeenCalledWith('agent crashed');
  });
});

import { describe, it, expect, vi, afterEach } from 'vitest';
import { readTextAttachmentPreview, TEXT_PREVIEW_MAX_BYTES } from '../textAttachmentPreview';
import { discussions, setApiBase } from '../api';

const bytes = (text: string) => new TextEncoder().encode(text);
function stream(chunks: Uint8Array[], cancel = vi.fn()) {
  return new ReadableStream<Uint8Array>({
    pull(controller) {
      const chunk = chunks.shift();
      if (chunk) controller.enqueue(chunk);
      else controller.close();
    },
    cancel,
  });
}

afterEach(() => vi.unstubAllGlobals());

describe('bounded text attachment preview', () => {
  it('preserves JSON and Unicode split between chunks', async () => {
    const text = '{"été":"🗼"}\n';
    const chunks = Array.from(bytes(text), byte => new Uint8Array([byte]));
    await expect(readTextAttachmentPreview(stream(chunks))).resolves.toEqual({ text, truncated: false });
  });

  it('handles an empty file and a file exactly at the limit', async () => {
    await expect(readTextAttachmentPreview(stream([]))).resolves.toEqual({ text: '', truncated: false });
    const text = 'x'.repeat(TEXT_PREVIEW_MAX_BYTES);
    await expect(readTextAttachmentPreview(stream([bytes(text)]))).resolves.toEqual({ text, truncated: false });
  });

  it('cuts at the byte limit without corrupting a Unicode character and cancels the reader', async () => {
    const cancel = vi.fn();
    const text = 'x'.repeat(TEXT_PREVIEW_MAX_BYTES - 1);
    const result = await readTextAttachmentPreview(stream([bytes(text + '🗼tail'), bytes('unread')], cancel));
    expect(result).toEqual({ text, truncated: true });
    expect(cancel).toHaveBeenCalledOnce();
  });

  it('detects overflow when the boundary coincides with a chunk boundary', async () => {
    const text = 'x'.repeat(TEXT_PREVIEW_MAX_BYTES);
    await expect(readTextAttachmentPreview(stream([bytes(text), bytes('tail')]))).resolves.toEqual({ text, truncated: true });
  });

  it.each([new Uint8Array([0]), new Uint8Array([0xff]), new Uint8Array([0xe2, 0x82])])(
    'refuses binary or invalid UTF-8 bytes', async value => {
      await expect(readTextAttachmentPreview(stream([value]))).rejects.toThrow();
    },
  );

  it('propagates stream failures', async () => {
    const body = new ReadableStream<Uint8Array>({ start(controller) { controller.error(new Error('disconnected')); } });
    await expect(readTextAttachmentPreview(body)).rejects.toThrow('disconnected');
  });

  it('uses the authenticated API content route and forwards cancellation', async () => {
    const fetch = vi.fn().mockResolvedValue({ ok: true, body: stream([bytes('log')]) });
    vi.stubGlobal('fetch', fetch);
    setApiBase('');
    const controller = new AbortController();
    await expect(discussions.contextFileTextPreview('disc-1', 'file-1', controller.signal)).resolves.toEqual({ text: 'log', truncated: false });
    expect(fetch).toHaveBeenCalledWith('/api/discussions/disc-1/context-files/file-1/content', {
      headers: expect.any(Object), signal: controller.signal,
    });
    fetch.mockResolvedValueOnce({ ok: false, status: 404 });
    await expect(discussions.contextFileTextPreview('d', 'f')).rejects.toThrow('404');
  });
});

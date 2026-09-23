/** Bound preview decoding independently of the original download. */
export const TEXT_PREVIEW_MAX_BYTES = 256 * 1024;

export type TextAttachmentPreview = { text: string; truncated: boolean };

export async function readTextAttachmentPreview(
  body: ReadableStream<Uint8Array> | null,
): Promise<TextAttachmentPreview> {
  if (!body) return { text: '', truncated: false };
  const reader = body.getReader();
  const decoder = new TextDecoder('utf-8', { fatal: true });
  let text = '';
  let size = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) return { text: text + decoder.decode(), truncated: false };
      const remaining = TEXT_PREVIEW_MAX_BYTES - size;
      const part = value.subarray(0, remaining);
      text += decoder.decode(part, { stream: true });
      size += part.byteLength;
      if (text.includes('\0')) throw new Error('Attachment is not UTF-8 text');
      // Keep a partial UTF-8 character buffered when cutting at the limit.
      // Never flush it as a replacement glyph or fail a valid large file.
      if (value.byteLength > remaining) return { text, truncated: true };
    }
  } finally {
    await reader.cancel().catch(() => undefined);
    reader.releaseLock();
  }
}

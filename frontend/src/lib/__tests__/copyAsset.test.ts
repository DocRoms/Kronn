import { describe, expect, it, vi } from 'vitest';
import { COPY_TEXT_MAX_BYTES, copyAsset, CopyAssetError, copyRefusal, type CopyEnv } from '../copyAsset';

class FakeClipboardItem {
  static accepted = new Set(['image/png', 'text/plain']);
  static supports(type: string) { return FakeClipboardItem.accepted.has(type); }
  constructor(readonly items: Record<string, Blob | Promise<Blob>>) {}
}

/** Like a browser: the write settles once every item payload has resolved. */
const clipboard = () => {
  const written: Record<string, Blob>[] = [];
  return {
    written,
    writeText: vi.fn().mockResolvedValue(undefined),
    write: vi.fn(async (items: FakeClipboardItem[]) => {
      for (const item of items) {
        const entry: Record<string, Blob> = {};
        for (const [type, value] of Object.entries(item.items)) entry[type] = await value;
        written.push(entry);
      }
    }),
  };
};

const env = (over: Partial<CopyEnv> = {}) => {
  const board = clipboard();
  return {
    board,
    env: {
      secure: true,
      clipboard: board as unknown as CopyEnv['clipboard'],
      ClipboardItem: FakeClipboardItem as unknown as CopyEnv['ClipboardItem'],
      toPng: vi.fn(async () => new Blob(['png'], { type: 'image/png' })),
      ...over,
    } satisfies CopyEnv,
  };
};

const reason = (promise: Promise<unknown>) =>
  promise.then(() => null, (e: unknown) => (e instanceof CopyAssetError ? e.reason : 'other'));

describe('copyAsset', () => {
  it('starts the clipboard write before the download finishes, then fills it', async () => {
    const { board, env: e } = env();
    let finish!: (blob: Blob) => void;
    const pending = copyAsset('text', 'text/plain', () => new Promise(resolve => { finish = resolve; }), e);
    expect(board.write).toHaveBeenCalledTimes(1);
    const body = 'x'.repeat(300 * 1024);
    finish(new Blob([body]));
    await pending;
    expect(await board.written[0]['text/plain'].text()).toBe(body);
  });

  it('refuses a text file too large for the clipboard and says so', async () => {
    const { board, env: e } = env();
    const big = new Blob([new Uint8Array(COPY_TEXT_MAX_BYTES + 1)]);
    expect(await reason(copyAsset('text', 'text/plain', async () => big, e))).toBe('too_large');
    expect(board.written).toHaveLength(0);
  });

  it('falls back to writeText for text when ClipboardItem is missing', async () => {
    const { board, env: e } = env({ ClipboardItem: undefined });
    await copyAsset('text', 'text/plain', async () => new Blob(['plain']), e);
    expect(board.writeText).toHaveBeenCalledWith('plain');
  });

  it('writes a PNG as is and re-encodes any other image', async () => {
    const { board, env: e } = env();
    await copyAsset('image', 'image/png', async () => new Blob(['a'], { type: 'image/png' }), e);
    expect(e.toPng).not.toHaveBeenCalled();
    await copyAsset('image', 'image/jpeg', async () => new Blob(['b'], { type: 'image/jpeg' }), e);
    expect(e.toPng).toHaveBeenCalledTimes(1);
    expect(board.written.map(Object.keys)).toEqual([['image/png'], ['image/png']]);
  });

  it('only offers video where the browser accepts that exact type', async () => {
    expect(copyRefusal('video', 'video/mp4', env().env)).toBe('video_unsupported');
    FakeClipboardItem.accepted.add('video/mp4');
    try {
      const { board, env: e } = env();
      await copyAsset('video', 'video/mp4', async () => new Blob(['v'], { type: 'video/mp4' }), e);
      expect(board.written.map(Object.keys)).toEqual([['video/mp4']]);
    } finally {
      FakeClipboardItem.accepted.delete('video/mp4');
    }
  });

  it('refuses before downloading in an insecure context or without ClipboardItem for media', async () => {
    const load = vi.fn(async () => new Blob(['x']));
    expect(await reason(copyAsset('text', 'text/plain', load, env({ secure: false }).env))).toBe('insecure');
    expect(await reason(copyAsset('image', 'image/png', load, env({ ClipboardItem: undefined }).env))).toBe('unsupported');
    expect(load).not.toHaveBeenCalled();
  });

  it('reports a browser refusal instead of an unexplained failure', async () => {
    const { env: e } = env();
    (e.clipboard as unknown as { write: ReturnType<typeof vi.fn> }).write = vi.fn().mockRejectedValue(new Error('NotAllowedError'));
    expect(await reason(copyAsset('image', 'image/png', async () => new Blob(['x'], { type: 'image/png' }), e))).toBe('denied');
  });
});

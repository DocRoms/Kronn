// KT-556 — the guard that matters: a frame that was never decoded must not
// leave this module. Shipping a black rectangle as "the last image" would be
// silent, exportable and wrong, and it would then be paid for as the starting
// picture of a follow-up clip.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { extractLastFrame, LastFrameError, lastFrameFilename } from '../lastFrame';

type FakeVideo = {
  duration: number;
  videoWidth: number;
  videoHeight: number;
  readyState: number;
  currentTime: number;
  preload: string;
  muted: boolean;
  playsInline: boolean;
  src: string;
  addEventListener: (event: string, handler: () => void, options?: unknown) => void;
  removeEventListener: (event: string, handler: () => void) => void;
  removeAttribute: (name: string) => void;
  load: () => void;
  __fire: (event: string) => void;
};

function fakeVideo(over: Partial<FakeVideo> = {}): FakeVideo {
  const handlers = new Map<string, Set<() => void>>();
  const video: FakeVideo = {
    duration: 4,
    videoWidth: 640,
    videoHeight: 640,
    readyState: 4,
    currentTime: 0,
    preload: '',
    muted: false,
    playsInline: false,
    src: '',
    addEventListener: (event, handler) => {
      if (!handlers.has(event)) handlers.set(event, new Set());
      handlers.get(event)!.add(handler);
    },
    removeEventListener: (event, handler) => { handlers.get(event)?.delete(handler); },
    removeAttribute: vi.fn(),
    load: vi.fn(),
    __fire: (event) => { for (const handler of [...(handlers.get(event) ?? [])]) handler(); },
    ...over,
  };
  // Metadata is announced on the next tick, and the seek answers as soon as it
  // is requested — the real element never resolves both in the same turn.
  queueMicrotask(() => video.__fire('loadedmetadata'));
  let sought = 0;
  Object.defineProperty(video, 'currentTime', {
    set(value: number) { sought = value; queueMicrotask(() => video.__fire('seeked')); },
    get() { return sought; },
    configurable: true,
  });
  return video;
}

/** A canvas whose pixels are whatever the test says the decode produced. */
function fakeCanvas(pixels: number[], blob: Blob | null = new Blob(['png'], { type: 'image/png' })) {
  return {
    width: 0,
    height: 0,
    getContext: () => ({
      drawImage: vi.fn(),
      getImageData: () => ({ data: Uint8ClampedArray.from(pixels) }),
    }),
    toBlob: (cb: (b: Blob | null) => void) => cb(blob),
  };
}

/** Two visibly different pixels: what a real decode looks like. */
const DECODED = [10, 20, 30, 255, 200, 90, 40, 255];
/** One value everywhere: indistinguishable from having decoded nothing. */
const BLANK = [0, 0, 0, 255, 0, 0, 0, 255];

function install(video: unknown, canvas: unknown) {
  vi.spyOn(document, 'createElement').mockImplementation(((tag: string) => {
    if (tag === 'video') return video;
    if (tag === 'canvas') return canvas;
    throw new Error(`unexpected createElement(${tag})`);
  }) as never);
}

describe('extractLastFrame', () => {
  beforeEach(() => vi.restoreAllMocks());

  it('draws the end of the clip, not its start', async () => {
    const video = fakeVideo({ duration: 4 });
    const canvas = fakeCanvas(DECODED);
    install(video, canvas);

    const frame = await extractLastFrame('blob:clip');

    expect(frame.width).toBe(640);
    expect(frame.height).toBe(640);
    expect(frame.blob.size).toBeGreaterThan(0);
    expect(canvas.width).toBe(640);
    // Seeking to `duration` exactly lands past the last decoded frame on
    // several browsers and paints nothing; a hair before it is inside it.
    expect(video.currentTime).toBeLessThan(4);
    expect(video.currentTime).toBeGreaterThan(3.9);
  });

  it('refuses a blank canvas rather than calling it the last frame', async () => {
    install(fakeVideo(), fakeCanvas(BLANK));
    await expect(extractLastFrame('blob:clip')).rejects.toMatchObject({ cause_: 'decode' });
  });

  it('refuses when the seek completed but no frame is available to draw', async () => {
    // `seeked` fires on the seek, not on the decode: readyState is the only
    // thing that says a picture exists.
    install(fakeVideo({ readyState: 1 }), fakeCanvas(DECODED));
    await expect(extractLastFrame('blob:clip')).rejects.toMatchObject({ cause_: 'decode' });
  });

  it('refuses a clip that announces no duration', async () => {
    install(fakeVideo({ duration: Number.NaN }), fakeCanvas(DECODED));
    await expect(extractLastFrame('blob:clip')).rejects.toBeInstanceOf(LastFrameError);
  });

  it('refuses a clip that announces no dimensions', async () => {
    install(fakeVideo({ videoWidth: 0, videoHeight: 0 }), fakeCanvas(DECODED));
    await expect(extractLastFrame('blob:clip')).rejects.toMatchObject({ cause_: 'metadata' });
  });

  it('reports an encode failure instead of returning an empty file', async () => {
    install(fakeVideo(), fakeCanvas(DECODED, null));
    await expect(extractLastFrame('blob:clip')).rejects.toMatchObject({ cause_: 'encode' });
  });

  it('releases the decoder whatever happened', async () => {
    const video = fakeVideo();
    install(video, fakeCanvas(BLANK));
    await expect(extractLastFrame('blob:clip')).rejects.toBeInstanceOf(LastFrameError);
    // The caller revokes the object URL right after: a video still holding it
    // keeps decoding bytes that are about to disappear.
    expect(video.removeAttribute).toHaveBeenCalledWith('src');
    expect(video.load).toHaveBeenCalled();
  });
});

describe('lastFrameFilename', () => {
  it('says which clip the picture came from', () => {
    expect(lastFrameFilename('seedance-clip.mp4')).toBe('seedance-clip-last-frame.png');
  });

  it('handles a name without an extension', () => {
    expect(lastFrameFilename('clip')).toBe('clip-last-frame.png');
  });
});

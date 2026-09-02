// KT-556 — the last picture of a clip, decoded by the browser.
//
// The backend cannot do this: the clips these providers return are H.264
// profile 100 (High), which the pure-Rust decoder available here reads for 9
// frames out of 97 before failing, and ffmpeg is on neither the machine nor
// the repo. The player in the viewer decodes them without trouble, and this
// path only ever runs from a click in that same viewer — so the element that
// already succeeds is the one asked to do the work.

/** Why an extraction could not produce a trustworthy frame. */
export type LastFrameFailure =
  | 'metadata'   // duration/dimensions never became known
  | 'seek'       // the seek to the end never completed
  | 'decode'     // nothing was drawn: zero-sized or fully blank canvas
  | 'encode';    // the canvas refused to serialise

export class LastFrameError extends Error {
  constructor(readonly cause_: LastFrameFailure) {
    super(`last-frame extraction failed: ${cause_}`);
    this.name = 'LastFrameError';
  }
}

export type LastFrame = { blob: Blob; width: number; height: number };

/** Giving up beats hanging on a clip the browser silently refuses to decode. */
const STEP_TIMEOUT_MS = 15_000;
/** Seeking to `duration` exactly lands past the last decoded frame on several
 *  browsers and paints nothing; a hair before it is inside the frame. */
const END_EPSILON_S = 0.05;

function once(target: EventTarget, event: string, timeoutMs: number, failure: LastFrameFailure): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { cleanup(); reject(new LastFrameError(failure)); }, timeoutMs);
    const onEvent = () => { cleanup(); resolve(); };
    const onError = () => { cleanup(); reject(new LastFrameError(failure)); };
    function cleanup() {
      clearTimeout(timer);
      target.removeEventListener(event, onEvent);
      target.removeEventListener('error', onError);
    }
    target.addEventListener(event, onEvent, { once: true });
    target.addEventListener('error', onError, { once: true });
  });
}

/** True when every pixel carries the same value: a decode that produced a
 *  uniform rectangle is indistinguishable from one that produced nothing, and
 *  shipping it as "the last frame" is exactly the failure this guards. */
function isBlank(data: Uint8ClampedArray): boolean {
  if (data.length < 4) return true;
  for (let i = 4; i < data.length; i += 4) {
    if (data[i] !== data[0] || data[i + 1] !== data[1] || data[i + 2] !== data[2]) return false;
  }
  return true;
}

/**
 * Decode `src` (an object URL for the clip's authenticated bytes) and return
 * its last frame as a PNG.
 *
 * Throws `LastFrameError` rather than returning a black rectangle: a frame
 * that was never decoded must not reach a billable generation as a starting
 * picture.
 */
export async function extractLastFrame(src: string): Promise<LastFrame> {
  const video = document.createElement('video');
  video.preload = 'auto';
  video.muted = true;
  video.playsInline = true;
  video.src = src;

  try {
    await once(video, 'loadedmetadata', STEP_TIMEOUT_MS, 'metadata');
    const duration = video.duration;
    if (!Number.isFinite(duration) || duration <= 0) throw new LastFrameError('metadata');
    if (!video.videoWidth || !video.videoHeight) throw new LastFrameError('metadata');

    const seeked = once(video, 'seeked', STEP_TIMEOUT_MS, 'seek');
    video.currentTime = Math.max(0, duration - END_EPSILON_S);
    await seeked;
    // `seeked` fires on the seek, not on the decode. HAVE_CURRENT_DATA is the
    // first state in which a frame is actually available to draw.
    if (video.readyState < 2) throw new LastFrameError('decode');

    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    if (!canvas.width || !canvas.height) throw new LastFrameError('decode');
    const context = canvas.getContext('2d');
    if (!context) throw new LastFrameError('decode');
    context.drawImage(video, 0, 0, canvas.width, canvas.height);

    const pixels = context.getImageData(0, 0, canvas.width, canvas.height);
    if (isBlank(pixels.data)) throw new LastFrameError('decode');

    const blob = await new Promise<Blob | null>(resolve => canvas.toBlob(resolve, 'image/png'));
    if (!blob || blob.size === 0) throw new LastFrameError('encode');
    return { blob, width: canvas.width, height: canvas.height };
  } finally {
    // Drop the decoder before the caller revokes the object URL.
    video.removeAttribute('src');
    video.load();
  }
}

/** `clip.mp4` → `clip-last-frame.png`, so the library says where it came from. */
export function lastFrameFilename(videoFilename: string): string {
  const stem = videoFilename.replace(/\.[^./\\]+$/, '') || 'video';
  return `${stem}-last-frame.png`;
}

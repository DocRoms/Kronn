/**
 * Browser E2E — the last frame really is the LAST one, decoded by a real
 * browser (KT-556).
 *
 * The unit tests drive a fake `<video>`: they prove the guards fire, not that
 * anything decodes. jsdom decodes no video at all, and the whole reason this
 * feature lives in the browser is that the backend cannot read these clips —
 * H.264 profile 100, 9 frames out of 97 through the pure-Rust decoder, and no
 * ffmpeg on the machine or in the repo.
 *
 * So the clip here is encoded by Chromium itself and read back by Chromium:
 * red for the first half, green for the second. An extractor that returned
 * the first decoded frame — the exact failure this feature was written to
 * avoid — comes back RED and fails this spec. Only a real seek to the end
 * comes back green.
 */
import { test, expect } from '../fixtures/kronn-fixture';

const WIDTH = 640;
const HEIGHT = 360;

test.describe('last frame extraction', () => {
  test('returns the end of the clip, not its beginning', async ({ page }) => {
    await page.goto('/');

    const result = await page.evaluate(async ({ width, height }) => {
      const { extractLastFrame } = await import('/src/lib/lastFrame.ts');

      // ── Encode a clip whose two halves are visibly different ──
      const source = document.createElement('canvas');
      source.width = width;
      source.height = height;
      const paint = source.getContext('2d')!;
      // A flat rectangle is exactly what a failed decode looks like, and the
      // extractor rejects it on purpose — so the clip carries a marker that
      // makes every frame non-uniform.
      const frameOf = (background: string) => {
        paint.fillStyle = background;
        paint.fillRect(0, 0, width, height);
        paint.fillStyle = 'rgb(10, 10, 190)';
        paint.fillRect(0, 0, Math.floor(width / 8), Math.floor(height / 8));
      };
      frameOf('rgb(220, 20, 20)');

      const stream = source.captureStream(30);
      const chunks: Blob[] = [];
      const recorder = new MediaRecorder(stream);
      recorder.ondataavailable = event => { if (event.data.size) chunks.push(event.data); };
      const stopped = new Promise<void>(resolve => { recorder.onstop = () => resolve(); });
      recorder.start();

      const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));
      // Repainting keeps frames flowing: a still canvas produces a stream the
      // encoder may collapse to a single frame, and the seek would then have
      // nothing to land on.
      const paintFor = async (color: string, ms: number) => {
        const until = performance.now() + ms;
        while (performance.now() < until) {
          frameOf(color);
          await wait(33);
        }
      };
      await paintFor('rgb(220, 20, 20)', 700);
      await paintFor('rgb(20, 200, 60)', 700);
      recorder.stop();
      await stopped;

      const clip = new Blob(chunks, { type: chunks[0]?.type || 'video/webm' });
      const clipUrl = URL.createObjectURL(clip);
      try {
        const frame = await extractLastFrame(clipUrl);

        // ── Read the colour back out of the produced PNG ──
        const bitmap = await createImageBitmap(frame.blob);
        const check = document.createElement('canvas');
        check.width = bitmap.width;
        check.height = bitmap.height;
        check.getContext('2d')!.drawImage(bitmap, 0, 0);
        const [r, g, b] = check.getContext('2d')!
          .getImageData(Math.floor(bitmap.width / 2), Math.floor(bitmap.height / 2), 1, 1).data;
        return { width: frame.width, height: frame.height, size: frame.blob.size, r, g, b };
      } finally {
        URL.revokeObjectURL(clipUrl);
      }
    }, { width: WIDTH, height: HEIGHT });

    expect(result.width).toBe(WIDTH);
    expect(result.height).toBe(HEIGHT);
    expect(result.size).toBeGreaterThan(0);
    // Green is the second half. Red would mean the extractor handed back an
    // early frame and called it the last one.
    expect(result.g).toBeGreaterThan(result.r + 40);
    expect(result.g).toBeGreaterThan(result.b + 40);
  });

  test('refuses a file that decodes to nothing', async ({ page }) => {
    await page.goto('/');

    const failure = await page.evaluate(async () => {
      const { extractLastFrame } = await import('/src/lib/lastFrame.ts');
      const notAVideo = URL.createObjectURL(new Blob(['this is not a clip'], { type: 'video/mp4' }));
      try {
        await extractLastFrame(notAVideo);
        return 'resolved';
      } catch (e) {
        return (e as { cause_?: string }).cause_ ?? 'unknown';
      } finally {
        URL.revokeObjectURL(notAVideo);
      }
    });

    // A file the browser cannot open must fail loudly, never produce a black
    // rectangle a follow-up generation would then be billed for.
    expect(failure).toBe('metadata');
  });
});

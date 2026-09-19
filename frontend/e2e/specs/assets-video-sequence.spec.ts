/**
 * KT-682 — the clips of a discussion, ordered, sorted and played as one film.
 *
 * What no unit test can prove: that the order and the clips set aside survive
 * a reload because the server kept them, that a real browser draws each clip's
 * first frame, and that it chains real clips, each one starting when the
 * previous one ends. The clips are recorded by the browser itself (WebM, no
 * provider, no cost), so their bytes are genuine video.
 */
import { expect, test } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';
import { openAssetsPanel } from '../pages/MediaLauncher';

const createdDiscussionIds = new Set<string>();

test.afterEach(async ({ request }) => {
  for (const id of createdDiscussionIds) {
    await request.delete(`/api/discussions/${id}`).catch(() => {});
  }
  createdDiscussionIds.clear();
});

test('orders the clips of a discussion, sets one aside and plays the others in turn', async ({ page, request }) => {
  test.setTimeout(90_000);

  const discussion = await request.post('/api/discussions', {
    data: {
      title: `KT-682 final cut ${Date.now()}`,
      agent: 'Codex',
      language: 'fr',
      initial_prompt: 'Discussion de test KT-682, sans agent.',
      no_agent: true,
    },
  });
  expect(discussion.ok()).toBe(true);
  const discussionId = (await discussion.json())?.data?.id as string;
  expect(discussionId).toBeTruthy();
  createdDiscussionIds.add(discussionId);

  const dashboard = new DashboardPage(page);
  await dashboard.goto();

  // Three short clips of different colours, recorded from a canvas.
  const recorded = await page.evaluate(async () => {
    async function record(colour: string): Promise<string> {
      const canvas = document.createElement('canvas');
      canvas.width = 160;
      canvas.height = 90;
      const paint = canvas.getContext('2d');
      if (!paint) throw new Error('this browser cannot paint a canvas');
      const recorder = new MediaRecorder(canvas.captureStream(15), { mimeType: 'video/webm' });
      const chunks: Blob[] = [];
      recorder.ondataavailable = event => chunks.push(event.data);
      const stopped = new Promise(done => { recorder.onstop = done; });
      recorder.start();
      const until = performance.now() + 800;
      while (performance.now() < until) {
        paint.fillStyle = colour;
        paint.fillRect(0, 0, 160, 90);
        paint.fillStyle = 'white';
        paint.fillText(String(Math.round(until - performance.now())), 10, 50);
        await new Promise(frame => requestAnimationFrame(frame));
      }
      recorder.stop();
      await stopped;
      const bytes = new Uint8Array(await new Blob(chunks, { type: 'video/webm' }).arrayBuffer());
      let binary = '';
      for (const byte of bytes) binary += String.fromCharCode(byte);
      return btoa(binary);
    }
    return [
      await record('rgb(200, 60, 40)'),
      await record('rgb(40, 90, 200)'),
      await record('rgb(40, 160, 90)'),
    ];
  });

  const ids: string[] = [];
  for (const [index, name] of ['plan-a.webm', 'plan-b.webm', 'hors-sujet.webm'].entries()) {
    const upload = await request.post(`/api/discussions/${discussionId}/context-files`, {
      multipart: {
        file: { name, mimeType: 'video/webm', buffer: Buffer.from(recorded[index], 'base64') },
      },
    });
    expect(upload.ok()).toBe(true);
    const id = (await upload.json())?.data?.file?.id as string;
    expect(id).toBeTruthy();
    ids.push(id);
  }

  await dashboard.openDiscussion(discussionId);
  let panel = await openAssetsPanel(page);
  const film = () => panel.getByTestId('video-sequence-final').locator('.disc-sequence-name');
  const setAside = () => panel.getByTestId('video-sequence-aside').locator('.disc-sequence-name');
  const saved = () => page.waitForResponse(response =>
    response.url().endsWith(`/api/discussions/${discussionId}/video-sequence`)
    && response.request().method() === 'PUT');

  await expect(panel.getByTestId('assets-tab-editor')).toContainText('3');
  await panel.getByTestId('assets-tab-editor').click();
  await expect(film()).toHaveText(['plan-a.webm', 'plan-b.webm', 'hors-sujet.webm']);

  // A real drag: before anything is dropped, a line shows where the clip
  // will land, and the clip being moved fades.
  const row = (name: string) => panel.getByTestId('video-sequence-item').filter({ hasText: name });
  await row('plan-b.webm').hover();
  await page.mouse.down();
  const target = await row('plan-a.webm').boundingBox();
  if (!target) throw new Error('plan-a.webm has no box on screen');
  await page.mouse.move(target.x + target.width / 2, target.y + 6, { steps: 8 });
  await expect(row('plan-a.webm')).toHaveAttribute('data-drop', 'before');
  await expect(row('plan-b.webm')).toHaveAttribute('data-dragging', 'true');
  await page.screenshot({ path: 'test-results/kt682-drag.png' });
  let save = saved();
  await page.mouse.up();
  expect((await save).ok()).toBe(true);
  await expect(film()).toHaveText(['plan-b.webm', 'plan-a.webm', 'hors-sujet.webm']);
  await expect(panel.locator('[data-drop]')).toHaveCount(0);

  // The unrelated clip is dragged out of the film.
  save = saved();
  await panel.getByTestId('video-sequence-item').filter({ hasText: 'hors-sujet.webm' })
    .dragTo(panel.getByTestId('video-sequence-aside'));
  expect((await save).ok()).toBe(true);
  await expect(film()).toHaveText(['plan-b.webm', 'plan-a.webm']);
  await expect(setAside()).toHaveText(['hors-sujet.webm']);

  // Kept by the server, not by the page.
  const stored = await request.get(`/api/discussions/${discussionId}/video-sequence`);
  const sequence = (await stored.json())?.data;
  expect(sequence?.file_ids).toEqual([ids[1], ids[0]]);
  expect(sequence?.excluded_ids).toEqual([ids[2]]);
  await page.reload();
  await dashboard.openDiscussion(discussionId);
  panel = await openAssetsPanel(page);
  await panel.getByTestId('assets-tab-editor').click();
  await expect(film()).toHaveText(['plan-b.webm', 'plan-a.webm']);
  await expect(setAside()).toHaveText(['hors-sujet.webm']);

  // Uploads have no price: the total says so rather than showing $0.00.
  const totalCost = panel.getByTestId('video-sequence-total-cost');
  await expect(totalCost).toContainText('—');
  await expect(totalCost).toContainText('+ 2 clip(s) non compté(s)');

  // Each clip shows its first frame, decoded by the browser.
  const thumbs = panel.getByTestId('video-sequence-thumb').locator('video');
  await expect(thumbs).toHaveCount(3);
  await expect.poll(() => thumbs.evaluateAll(videos =>
    videos.every(video => (video as HTMLVideoElement).readyState >= 2))).toBe(true);
  await page.screenshot({ path: 'test-results/kt682-editor.png' });

  await panel.getByTestId('video-sequence-play').click();
  const player = page.getByTestId('video-sequence-player');
  const position = player.getByTestId('video-sequence-position');
  // Two clips, not three: the one set aside is not played.
  await expect(position).toContainText('1/2');
  await expect(position).toContainText('plan-b.webm');
  await expect(player.getByTestId('video-sequence-video')).toBeVisible();
  await page.screenshot({ path: 'test-results/kt682-player.png' });
  // Nobody touches the player: the second clip starts on its own.
  await expect(position).toContainText('2/2', { timeout: 15_000 });
  await expect(position).toContainText('plan-a.webm');
  await expect(player.getByTestId('video-sequence-replay')).toBeVisible({ timeout: 15_000 });

  await page.keyboard.press('Escape');
  await expect(player).toHaveCount(0);
});

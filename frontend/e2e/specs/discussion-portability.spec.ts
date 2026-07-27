/**
 * Portable discussion bundle — real backend + UI.
 *
 * Covers the human path rather than only the API contract: export from the
 * discussion header, import from the sidebar, then replay the exact same
 * bundle without creating another discussion.
 */

import { expect, test } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const title = `Portable discussion PW ${Date.now()}`;
const createdIds = new Set<string>();

test.afterAll(async ({ request }) => {
  for (const id of createdIds) {
    await request.delete(`/api/discussions/${id}`).catch(() => {});
  }
});

test('exports, imports and replays a discussion idempotently', async ({ page, request }) => {
  const created = await request.post('/api/discussions', {
    data: {
      title,
      agent: 'Codex',
      language: 'fr',
      initial_prompt: 'Message portable sans lancement de modèle.',
      no_agent: true,
    },
  });
  expect(created.ok()).toBe(true);
  const createdBody = await created.json();
  const sourceId = createdBody?.data?.id as string;
  expect(sourceId).toBeTruthy();
  createdIds.add(sourceId);

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.navDiscussions.click();
  await page.getByRole('button', { name: new RegExp(title) }).click();

  const downloadPromise = page.waitForEvent('download');
  await page.getByRole('button', { name: 'Exporter la discussion' }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toMatch(/\.kronn-discussion\.json$/);
  const bundlePath = await download.path();
  expect(bundlePath).toBeTruthy();

  const importButton = page.getByRole('button', { name: 'Importer une discussion' });
  await importButton.click();
  await page.locator('input[type="file"][accept*=".kronn-discussion.json"]').setInputFiles(bundlePath!);
  await expect(page.getByText(/Discussion importée : \d+ messages/)).toBeVisible();

  const afterImport = await request.get('/api/discussions');
  const afterImportBody = await afterImport.json();
  const matchingAfterImport = (afterImportBody?.data ?? []).filter(
    (discussion: { id: string; title: string }) => discussion.title === title,
  );
  expect(matchingAfterImport).toHaveLength(2);
  for (const discussion of matchingAfterImport) createdIds.add(discussion.id);

  await importButton.click();
  await page.locator('input[type="file"][accept*=".kronn-discussion.json"]').setInputFiles(bundlePath!);
  await expect(page.getByText(/Ce bundle avait déjà été importé/)).toBeVisible();

  const afterReplay = await request.get('/api/discussions');
  const afterReplayBody = await afterReplay.json();
  const matchingAfterReplay = (afterReplayBody?.data ?? []).filter(
    (discussion: { title: string }) => discussion.title === title,
  );
  expect(matchingAfterReplay).toHaveLength(2);
});

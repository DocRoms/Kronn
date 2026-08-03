/**
 * Explicit discussion ↔ CLI session ownership — real backend + UI.
 *
 * The source binding is distinct from a live participant: an external Claude
 * or Codex session can be the durable resume key while currently offline.
 */

import { expect, test } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const title = `Session binding PW ${Date.now()}`;
const sessionId = `claude-ui-${Date.now()}`;
let discId: string | null = null;

test.afterAll(async ({ request }) => {
  if (discId) {
    await request.post('/api/disc/unlink', { data: { disc_id: discId } }).catch(() => {});
    await request.delete(`/api/discussions/${discId}`).catch(() => {});
  }
});

test('links, displays, copies and unlinks an offline Claude session', async ({ page, request }) => {
  const created = await request.post('/api/discussions', {
    data: {
      title,
      agent: 'ClaudeCode',
      language: 'fr',
      initial_prompt: 'Ne lance aucun modèle.',
      no_agent: true,
    },
  });
  expect(created.ok()).toBe(true);
  const body = await created.json();
  expect(body?.success).toBe(true);
  discId = body?.data?.id;
  expect(discId).toBeTruthy();
  if (!discId) throw new Error('discussion creation returned no id');

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.openDiscussion(discId);

  await page.getByRole('button', { name: 'Session', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Session CLI liée' });
  await dialog.locator('select').selectOption('ClaudeCode');
  await dialog.locator('input').fill(sessionId);
  await dialog.getByRole('button', { name: 'Lier une session' }).click();

  await expect(page.getByRole('button', { name: /ClaudeCode · claude-u/ })).toBeVisible();
  await expect(dialog.getByText('Hors ligne ou non détectée')).toBeVisible();
  await expect(dialog.getByRole('button', { name: `Copier l’ID de session ${sessionId}` })).toBeVisible();

  const detail = await request.get(`/api/discussions/${discId}/source`);
  expect(detail.ok()).toBe(true);
  const detailBody = await detail.json();
  expect(detailBody?.data?.current).toMatchObject({
    binding_version: 1,
    source_agent: 'ClaudeCode',
    source_session_id: sessionId,
  });

  await dialog.getByRole('button', { name: 'Délier' }).click();
  await expect(page.getByRole('button', { name: 'Session', exact: true })).toBeVisible();
});

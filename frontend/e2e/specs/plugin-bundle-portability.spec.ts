import { expect, test } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const suffix = Date.now();
const label = `Portable Context7 PW ${suffix}`;
const createdConfigIds = new Set<string>();

test.afterAll(async ({ request }) => {
  for (const id of createdConfigIds) {
    await request.delete(`/api/mcps/configs/${id}`).catch(() => {});
  }
});

test('exports a selection and imports it again through the Plugins UI', async ({ page, request }) => {
  const created = await request.post('/api/mcps/configs', {
    data: {
      server_id: 'mcp-context7',
      label,
      env: {},
      args_override: [`--portable-e2e-${suffix}`],
      is_global: false,
      project_ids: [],
    },
  });
  expect(created.ok()).toBe(true);
  const createdBody = await created.json();
  expect(createdBody?.success, JSON.stringify(createdBody)).toBe(true);
  const sourceConfigId = createdBody.data.id as string;
  createdConfigIds.add(sourceConfigId);

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.navMcps.click();
  await page.getByRole('button', { name: 'Exporter', exact: true }).click();

  const exportDialog = page.getByRole('dialog', { name: 'Exporter des plugins' });
  const pluginRow = exportDialog.locator('label').filter({ hasText: label });
  await pluginRow.getByRole('checkbox').check();
  await exportDialog.getByRole('button', { name: 'Vérifier la sélection' }).click();
  await expect(exportDialog.getByText(/Export sûr/)).toBeVisible();
  const downloadPromise = page.waitForEvent('download');
  await exportDialog.getByRole('button', { name: /Télécharger le bundle/ }).click();
  const download = await downloadPromise;
  expect(download.suggestedFilename()).toMatch(/\.kronn-plugins\.json$/);
  const bundlePath = await download.path();
  expect(bundlePath).toBeTruthy();

  await request.delete(`/api/mcps/configs/${sourceConfigId}`);
  createdConfigIds.delete(sourceConfigId);
  await page.reload();
  await dashboard.navMcps.click();
  await page.getByRole('button', { name: 'Importer', exact: true }).click();

  const importDialog = page.getByRole('dialog', { name: 'Importer des plugins' });
  await importDialog.locator('input[type="file"]').setInputFiles(bundlePath!);
  await expect(importDialog.getByText(label)).toBeVisible();
  await importDialog.getByRole('button', { name: 'Importer le bundle' }).click();
  await expect(importDialog.getByText(/1 configuration\(s\) créée\(s\)/)).toBeVisible();

  const overview = await request.get('/api/mcps');
  const overviewBody = await overview.json();
  const imported = (overviewBody?.data?.configs ?? []).filter(
    (config: { id: string; label: string }) => config.label === label,
  );
  expect(imported).toHaveLength(1);
  createdConfigIds.add(imported[0].id);
});

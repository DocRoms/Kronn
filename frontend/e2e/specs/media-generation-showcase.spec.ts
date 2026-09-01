/**
 * KT-540 — screenshots of the media generation feature on real assets.
 *
 * Not an assertion suite: a capture pass over a discussion that already holds
 * a generated image and a generated clip, so the feature can be reviewed
 * without a screen share. It asserts only what a screenshot cannot show — that
 * the elements are actually there — and fails loudly rather than saving an
 * empty frame.
 *
 * Needs a backend on KRONN_BACKEND_URL holding MEDIA_DISC_ID.
 *
 * # Cost
 *
 * Zero $. No generation is launched here; the assets already exist.
 */
import { test, expect } from '@playwright/test';
import { mkdirSync } from 'node:fs';
import { DashboardPage } from '../pages/DashboardPage';

const DISC = process.env.MEDIA_DISC_ID ?? '';
const OUT = process.env.MEDIA_SHOT_DIR ?? 'test-results/media';
const CONNECTION = process.env.MEDIA_CONNECTION_ID ?? '';

test.beforeAll(() => {
  expect(DISC, 'MEDIA_DISC_ID must name a discussion holding generated assets').not.toBe('');
  mkdirSync(OUT, { recursive: true });
});

test.beforeEach(async ({ page }) => {
  // The onboarding tour's backdrop swallows every click on a fresh profile.
  await page.addInitScript(() => {
    try { localStorage.setItem('kronn:tour-completed', 'true'); } catch { /* incognito */ }
  });
});

test('captures the generated assets, the carousel and the launcher', async ({ page }) => {
  // A loaded dev machine (a parallel cargo build is enough) pushes first paint
  // well past the default budget; this pass is a capture, not a perf check.
  test.setTimeout(240_000);
  page.setDefaultTimeout(60_000);
  await page.setViewportSize({ width: 1440, height: 900 });
  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.openDiscussion(DISC);

  // The generated image rides in the discussion as an attachment of the
  // message that asked for it.
  const attachment = page.locator('.disc-msg-attachments img').first();
  await expect(attachment).toBeVisible({ timeout: 30_000 });
  await page.screenshot({ path: `${OUT}/01-discussion-with-asset.png`, fullPage: false });

  // Assets tab: the inventory of everything the discussion holds.
  await page.getByRole('button', { name: /Parcourir tous les assets/ }).click();
  const panel = page.getByRole('complementary', { name: 'Assets' });
  await expect(panel).toBeVisible();
  await expect(panel.getByTestId('discussion-asset-card').first()).toBeVisible({ timeout: 15_000 });
  await page.screenshot({ path: `${OUT}/02-assets-tab.png` });

  // The launcher — modality, connection, prompt, video shape, estimated price.
  await panel.getByRole('button', { name: /Générer un média/ }).click();
  const form = page.getByTestId('media-generate-form');
  await expect(form).toBeVisible();
  await expect(form.getByRole('combobox').first()).toBeVisible();
  await page.screenshot({ path: `${OUT}/03-launcher-image.png` });

  await form.getByRole('button', { name: 'Vidéo' }).click();
  await expect(form.getByText(/Durée/)).toBeVisible();
  await page.screenshot({ path: `${OUT}/04-launcher-video.png` });
  await panel.getByRole('button', { name: /Fermer le formulaire/ }).click();

  // Carousel: opening the image must reach the clip too — one sequence.
  await panel.locator('[data-testid="asset-library-grid"] img').first().click();
  const dialog = page.getByRole('dialog', { name: 'Visionneuse des médias joints' });
  await expect(dialog).toBeVisible();
  await page.screenshot({ path: `${OUT}/05-carousel-image.png` });

  // Walk with the on-screen control rather than the keyboard: Kronn binds
  // arrow keys globally for list navigation, so a key press is not a
  // trustworthy way to drive the carousel from a test.
  let sawVideo = false;
  for (let step = 0; step < 4 && !sawVideo; step += 1) {
    await dialog.getByRole('button', { name: 'Média suivant' }).click();
    sawVideo = await dialog
      .getByTestId('media-player-video')
      .waitFor({ state: 'visible', timeout: 10_000 })
      .then(() => true)
      .catch(() => false);
  }
  expect(sawVideo, 'the clip must be reachable from the same carousel as the image').toBe(true);
  await page.waitForTimeout(1200); // let the first frame paint
  await page.screenshot({ path: `${OUT}/06-carousel-video.png` });
});

test('captures the media slots on the connection card', async ({ page }) => {
  expect(CONNECTION, 'MEDIA_CONNECTION_ID must name the connection to open').not.toBe('');
  test.setTimeout(240_000);
  page.setDefaultTimeout(60_000);
  await page.setViewportSize({ width: 1440, height: 1000 });
  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.clickSettings();

  // Open the OpenRouter connection's editor, then frame the media block
  // itself: a full-page capture of Settings is unreadable.
  // The row carries the connection id in its test hook, which is stabler than
  // matching a display name that also appears in prose.
  const edit = page.getByTestId(`ext-api-edit-${CONNECTION}`);
  await expect(edit).toBeVisible({ timeout: 60_000 });
  await edit.scrollIntoViewIfNeeded();
  await edit.click();
  const form = page.getByTestId('ext-api-form');
  await expect(form).toBeVisible();
  const mediaPanel = page.getByTestId('ext-api-media-panel');
  await expect(mediaPanel).toBeVisible();
  await expect(page.getByTestId('ext-api-media-image')).toHaveValue(/gemini/);
  await expect(page.getByTestId('ext-api-media-video')).toHaveValue(/seedance/);
  await mediaPanel.scrollIntoViewIfNeeded();
  await form.screenshot({ path: `${OUT}/07-connection-media-slots.png` });
});

test('renders the launcher in both the dark and the light theme', async ({ page }) => {
  // The form's own CSS must ride the project token scale, not invented
  // variables: those resolve to nothing and the panel loses its borders and
  // background on whichever theme was not the one it was written under.
  expect(CONNECTION, 'MEDIA_CONNECTION_ID must name the connection to open').not.toBe('');
  test.setTimeout(240_000);
  page.setDefaultTimeout(60_000);
  await page.setViewportSize({ width: 1440, height: 900 });

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.openDiscussion(DISC);
  await page.getByRole('button', { name: /Parcourir tous les assets/ }).click();
  const panel = page.getByRole('complementary', { name: 'Assets' });
  await panel.getByRole('button', { name: /Générer un média/ }).click();
  const form = page.getByTestId('media-generate-form');
  await expect(form).toBeVisible();

  for (const theme of ['dark', 'light'] as const) {
    await page.evaluate((value) => {
      // 'dark' is the default :root, addressed by removing the attribute.
      if (value === 'dark') document.documentElement.removeAttribute('data-theme');
      else document.documentElement.setAttribute('data-theme', value);
    }, theme);
    // A token that resolved to nothing would leave this transparent.
    const background = await form.evaluate(
      (node) => getComputedStyle(node).backgroundColor,
    );
    expect(background, `the launcher must have a real background in ${theme}`)
      .not.toBe('rgba(0, 0, 0, 0)');
    const border = await form.evaluate((node) => getComputedStyle(node).borderTopColor);
    expect(border, `the launcher must have a real border in ${theme}`)
      .not.toBe('rgba(0, 0, 0, 0)');
    await form.screenshot({ path: `${OUT}/08-launcher-${theme}.png` });
  }
});

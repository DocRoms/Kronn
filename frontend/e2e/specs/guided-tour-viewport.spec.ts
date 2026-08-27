/**
 * Guided tour — walkable from end to end, with the card always on screen.
 *
 * KT-117. Reported by a human: at the step spotlighting the project area the
 * tooltip was off-viewport, so its only buttons were unreachable and the tour
 * could not be finished. The unit tests could not see it — happy-dom applies no
 * CSS and computes no layout, so a card pushed below the fold measures fine.
 * This walks every step in a real browser and asserts on real geometry.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import type { Page, Request } from '@playwright/test';
import { TOUR_STEPS } from '../../src/components/tour/tourSteps';

const VIEWPORTS: Array<{
  name: string;
  width: number;
  height: number;
}> = [
  { name: 'desktop', width: 1280, height: 800 },
  { name: 'laptop-court', width: 1280, height: 620 },
  // Half a screen: below the 768 px breakpoint, so the mobile layout applies,
  // but far wider than a phone. Kronn is not meant for phones — it IS meant to
  // survive a window docked to one side, which is where people actually work.
  // The mobile layout turns the discussions sidebar into a drawer. That absence is
  // declared on the step itself (`optionalWhenMissing`), so this spec no longer
  // repeats the fact: one truth, one place.
  { name: 'demi-ecran', width: 720, height: 900 },
];

/** Start the tour from a clean slate, as a first-time visitor would see it. */
async function startTour(page: Page) {
  await page.addInitScript(() => {
    localStorage.removeItem('kronn:tour-completed');
    localStorage.removeItem('kronn:tour-step');
    localStorage.removeItem('kronn:tour-progress:v1');
  });
  await page.goto('/');
  await expect(page.locator('.tour-tooltip')).toBeVisible({ timeout: 20_000 });
}

async function assertCardInsideViewport(page: Page, stepLabel: string) {
  const box = await page.locator('.tour-tooltip').boundingBox();
  expect(box, `${stepLabel}: the tour card must be laid out`).not.toBeNull();
  const size = page.viewportSize();
  if (!box || !size) return;
  // Fully inside — not merely intersecting. A card whose buttons sit past the
  // fold is exactly as unusable as one that is entirely off-screen.
  expect.soft(box.x, `${stepLabel}: card past the left edge`).toBeGreaterThanOrEqual(-1);
  expect.soft(box.y, `${stepLabel}: card past the top edge`).toBeGreaterThanOrEqual(-1);
  expect
    .soft(box.x + box.width, `${stepLabel}: card past the right edge`)
    .toBeLessThanOrEqual(size.width + 1);
  expect
    .soft(box.y + box.height, `${stepLabel}: card past the bottom edge`)
    .toBeLessThanOrEqual(size.height + 1);
}

/** Steps allowed to vanish by design, declared on the step and not per viewport. */
const OPTIONAL_STEPS = TOUR_STEPS.filter(step => step.optionalWhenMissing).length;

for (const viewport of VIEWPORTS) {
  test.describe(`Guided tour — ${viewport.name}`, () => {
    test.use({ viewport: { width: viewport.width, height: viewport.height } });

    // A full walk is inherently slow: each step that has to wait for its target
    // spends up to 15 s in the provider's bounded cross-page wait before moving on.
    test.setTimeout(240_000);

    test('every step keeps its card fully on screen and stays reachable', async ({ page }) => {
      await startTour(page);

      const counter = page.locator('.tour-step-counter');
      const total = Number((await counter.textContent())?.split('/')[1]?.trim() ?? '0');
      expect(total, 'the tour must expose a step count').toBeGreaterThan(5);

      const visited: string[] = [];
      for (let guard = 0; guard < total + 5; guard += 1) {
        const label = (await counter.textContent())?.trim() ?? `step ${guard}`;
        const title = (await page.locator('.tour-title').textContent())?.trim() ?? '?';
        visited.push(`${label} ${title}`);
        // Printed so a stall names the step it stalled on instead of just timing out.
        console.log(`[tour] ${label} — ${title}`);

        await assertCardInsideViewport(page, label);

        // The spotlight itself was never checked, so a degenerate one — a few
        // pixels wide in a corner because its target measured 0×0 — passed as
        // "fine" while the user saw no highlight at all.
        const stepId = await page.locator('.tour-tooltip').getAttribute('data-tour-step');
        const definition = TOUR_STEPS.find(step => step.id === stepId);
        const spot = page.locator('.tour-spotlight');
        if (definition?.selector) {
          await expect(spot, `${label}: a targeted step must render its spotlight`).toBeVisible({
            timeout: 2_000,
          });
        }
        const spotHandle = definition?.selector
          ? await spot.elementHandle({ timeout: 1_000 })
          : null;
        if (spotHandle) {
          // The spotlight animates for 0.35 s; reading mid-flight measured the
          // PREVIOUS step's rect and reported failures that were not real.
          let last = '';
          for (let settle = 0; settle < 12; settle += 1) {
            const b = await spotHandle.boundingBox();
            const key = b ? `${Math.round(b.width)}x${Math.round(b.height)}@${Math.round(b.x)},${Math.round(b.y)}` : 'none';
            if (key === last) break;
            last = key;
            await page.waitForTimeout(120);
          }
          const sb = await spotHandle.boundingBox();
          const vp = page.viewportSize();
          if (sb && vp) {
            expect.soft(sb.width, `${label}: spotlight too narrow to be a highlight`).toBeGreaterThan(24);
            expect.soft(sb.height, `${label}: spotlight too short to be a highlight`).toBeGreaterThan(16);
            expect.soft(sb.x, `${label}: spotlight starts off-screen`).toBeGreaterThan(-24);
            expect.soft(sb.y, `${label}: spotlight starts off-screen`).toBeGreaterThan(-24);
            if (stepId === 'agents-config') {
              const target = await page
                .locator('[data-tour-id="settings-agents"]')
                .boundingBox();
              expect(target, 'Agents step must point at the Agents accordion').not.toBeNull();
              if (target) {
                expect.soft(
                  Math.abs(sb.x - (target.x - 8)),
                  'Agents spotlight still points at the previous project button',
                ).toBeLessThanOrEqual(3);
                expect.soft(Math.abs(sb.y - (target.y - 8))).toBeLessThanOrEqual(3);
              }
            }
          }
        }
        if (stepId === 'copyable-ids') {
          await expect(
            page.locator('.tour-secondary-spotlight'),
            `${label}: message and discussion IDs must both be highlighted`,
          ).toBeVisible();
          await expect(
            page.locator('.tour-multi-spotlight-mask'),
            `${label}: both IDs must cut a clear hole in the dimmed overlay`,
          ).toBeVisible();
          await expect(page.locator('[data-tour-mask-hole]')).toHaveCount(2);
          await expect(page.locator('.tour-spotlight')).toHaveCSS('border-style', 'solid');
          await expect(page.locator('.tour-secondary-spotlight')).toHaveCSS('border-style', 'solid');
        }
        if (stepId === 'demo-render') {
          await expect(page.locator('.tour-desc')).toContainText('Vous pouvez l’agrandir');
        }
        if (stepId === 'global-search') {
          const result = page.locator('[data-tour-demo-result="true"]');
          await expect(result).toBeVisible();
          await expect(result).toContainText('Kronn · Demo');
          await expect(page.locator('[data-testid="global-search-input"]')).not.toHaveValue('');
        }

        // The new-discussion modal is opened by two steps and must not survive
        // them: left open it covers the pages the tour goes on to explain.
        if (stepId === 'disc-form') {
          const demoResponse = await page.request.post('/api/tour/demo-discussion');
          expect(demoResponse.ok(), 'the deterministic demo endpoint must answer').toBe(true);
          const demoPayload = await demoResponse.json() as {
            prompt?: string;
            data?: { prompt: string };
          };
          const prompt = demoPayload.prompt ?? demoPayload.data?.prompt;
          expect(prompt, 'the demo endpoint must expose the exact launcher prompt').toBeTruthy();
          const launcher = page.locator('.disc-new-card textarea');
          const partialPrompt = await launcher.inputValue();
          expect(
            partialPrompt.length,
            'the demo request must visibly type instead of appearing all at once',
          ).toBeLessThan(prompt!.length);
          await expect(launcher).toHaveValue(prompt!, { timeout: 10_000 });
        }
        if (stepId && !['new-disc', 'disc-form', 'mentions'].includes(stepId)) {
          await expect
            .soft(page.locator('.disc-new-overlay'), `${label} (${stepId}): the new-discussion modal is still open`)
            .toBeHidden();
        }

        // The way out must always be clickable — that is what "finishable" means.
        const skip = page.locator('.tour-btn-skip');
        await expect(skip, `${label}: Skip must stay visible`).toBeVisible();

        const nextBtn = page.locator('.tour-btn-next');
        await expect(nextBtn, `${label}: Next/Finish must stay visible`).toBeVisible();
        if (stepId === 'disc-form' && viewport.name === 'desktop') {
          // The launcher is the real form. During the tour its natural submit
          // action must advance to the already-seeded demo — never create a
          // second discussion or start an agent.
          const unsafeRequests: string[] = [];
          const recordUnsafeRequest = (request: Request) => {
            const url = new URL(request.url());
            if (
              request.method() === 'POST'
              && (
                url.pathname === '/api/discussions'
                || /^\/api\/discussions\/[^/]+\/run$/.test(url.pathname)
              )
            ) {
              unsafeRequests.push(`${request.method()} ${url.pathname}`);
            }
          };
          page.on('request', recordUnsafeRequest);
          const launchButton = page.locator('.disc-create-btn');
          // The E2E fixture intentionally has no configured agent, so the real
          // form disables this button. Enable only the DOM control here to
          // exercise the tour's capture handler; React's create handler must
          // still never receive the click.
          await launchButton.evaluate(button => button.removeAttribute('disabled'));
          await launchButton.dispatchEvent('click');
          await expect(counter).not.toHaveText(label, { timeout: 25_000 });
          await expect(page.locator('.disc-new-overlay')).toBeHidden();
          await page.waitForTimeout(250);
          page.off('request', recordUnsafeRequest);
          expect(unsafeRequests, 'the guided demo launch must stay agentless').toEqual([]);
          continue;
        }
        if (label.startsWith(`${total} /`)) {
          // Finishing must actually end the tour rather than loop back.
          const archiveDemo = page.waitForResponse(response => {
            const request = response.request();
            if (request.method() !== 'PATCH' || !request.url().includes('/api/discussions/')) {
              return false;
            }
            try {
              return JSON.parse(request.postData() ?? '{}').archived === true;
            } catch {
              return false;
            }
          });
          await nextBtn.click();
          await archiveDemo;
          await expect(page.locator('.tour-tooltip')).toBeHidden({ timeout: 10_000 });
          const finishedProgress = await page.evaluate(() =>
            JSON.parse(localStorage.getItem('kronn:tour-progress:v1') ?? '{}'),
          );
          expect(
            finishedProgress.completedStepIds,
            'Finish must account for every shown or explicitly optional step',
          ).toHaveLength(total);
          expect(finishedProgress.currentStepId).toBeNull();
          break;
        }

        // A step awaiting a real click drives the app; Next must still work so a
        // user is never trapped behind an interaction they cannot find.
        await nextBtn.click();
        try {
          await expect(counter).not.toHaveText(label, { timeout: 15_000 });
        } catch (error) {
          const diagnostics = await page.evaluate(() => ({
            progress: localStorage.getItem('kronn:tour-progress:v1'),
            displayedStep: document.querySelector('.tour-tooltip')
              ?.getAttribute('data-tour-step'),
            newDiscussionTarget: Boolean(document.querySelector(
              '[data-tour-id="new-disc-btn"]',
            )),
            settingsTarget: Boolean(document.querySelector(
              '[data-tour-id="settings-agents"]',
            )),
          }));
          throw new Error(`${String(error)}\nTour diagnostics: ${JSON.stringify(diagnostics)}`);
        }
      }

      // Leaving the tour must not leave the app covered by the tour's own modal.
      await expect(page.locator('.disc-new-overlay')).toBeHidden();

      // Pressing Next rather than performing each suggested click must not cost
      // the user any step: those explanations are the point of the tour.
      expect(
        visited.length,
        `the walk must cover every step reachable in this layout, visited: ${visited.join(' | ')}`,
      ).toBeGreaterThanOrEqual(total - OPTIONAL_STEPS);
    });
  });
}

test.describe('Guided tour — durable resume CTA', () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test('an interruption survives reload and resumes from the Settings card', async ({ page }) => {
    // Do not use startTour(): its persistent addInitScript intentionally clears
    // storage before every navigation, while this regression test must preserve
    // progress across a real reload.
    await page.addInitScript(() => {
      if (sessionStorage.getItem('tour-resume-test-initialized')) return;
      localStorage.clear();
      sessionStorage.setItem('tour-resume-test-initialized', 'true');
    });
    await page.goto('/');
    await expect(page.locator('.tour-tooltip')).toBeVisible({ timeout: 20_000 });
    await page.locator('.tour-btn-next').click();
    await expect(page.locator('.tour-step-counter')).toHaveText(`2 / ${TOUR_STEPS.length}`);

    await page.keyboard.press('Escape');
    await expect(page.locator('.tour-tooltip')).toBeHidden();

    await page.locator('[data-tour-id="nav-settings"]').click();
    const cta = page.locator('[data-testid="settings-tour-progress"]:visible');
    await expect(cta).toBeVisible();
    await expect(cta.locator('[role="progressbar"]')).toHaveAttribute('aria-valuenow', '1');

    await page.reload();
    await page.locator('[data-tour-id="nav-settings"]').click();
    await expect(cta).toBeVisible();
    const storedProgress = await page.evaluate(() =>
      JSON.parse(localStorage.getItem('kronn:tour-progress:v1') ?? '{}'),
    );
    expect(storedProgress).toMatchObject({
      completedStepIds: ['welcome'],
      currentStepId: 'navigation',
      hasStarted: true,
    });
    await cta.click();

    await expect(page.locator('.tour-tooltip')).toBeVisible();
    await expect(page.locator('.tour-step-counter')).toHaveText(`2 / ${TOUR_STEPS.length}`);
  });
});

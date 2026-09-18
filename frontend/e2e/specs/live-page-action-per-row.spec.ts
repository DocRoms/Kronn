/**
 * Live Page inline action — one block, one button per row (KT-678).
 *
 * The reported shape: a Page lists tickets and draws a "Framer" button on each
 * row from a single `application/kronn-action` block bound to the row with
 * `dynamic_binding`. After the first ticket's launch succeeded, clicking any
 * other ticket reopened the first one's card, marked succeeded, and nothing
 * ran. Unit tests cover the backend identity and the hook separately; this
 * spec drives the two together through the real sandboxed iframe, because the
 * failure needed both halves to agree on the wrong row.
 *
 * The target is a Quick Prompt, which reaches `succeeded` synchronously
 * without an installed agent CLI (see live-page-inline-action-launch.spec.ts).
 */

import { test, expect } from '../fixtures/kronn-fixture';

test.describe('Live Page inline action — each row is its own launch', () => {
  test('a second ticket launches its own run instead of replaying the first one', async ({ page }) => {
    const qpResponse = await page.request.post('/api/quick-prompts', {
      data: {
        name: `E2E frame per row ${Date.now()}`,
        prompt_template: 'Frame ticket {{ticket}}.',
        variables: [{ name: 'ticket', label: 'Ticket', placeholder: '' }],
      },
    });
    expect(qpResponse.ok()).toBe(true);
    const targetId = ((await qpResponse.json()) as { data: { id: string } }).data.id;

    const button = (ticket: string) =>
      `<button data-kronn-action="frame" data-kronn-bindings='{"ticket":"${ticket}"}'>Framer ${ticket}</button>`;
    const html = button('EW-7706') + button('EW-7704')
      + '<script type="application/kronn-action" data-action-id="frame">'
      + `{"kind":"quick_prompt","target_id":"${targetId}","values":[{"name":"ticket","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).key>"}]}`
      + '</script>';
    const pageResponse = await page.request.post('/api/pages', {
      data: {
        title: `E2E per-row actions ${Date.now()}`,
        html,
        datasets: [{ name: 'tickets', kind: 'collection', initial: [{ key: 'EW-7706' }, { key: 'EW-7704' }] }],
      },
    });
    expect(pageResponse.ok()).toBe(true);
    const pageId = ((await pageResponse.json()) as { data: { id: string } }).data.id;

    await page.goto(`/#page/${pageId}`);
    const frame = page.frameLocator('[data-testid="standalone-live-page-frame"]');
    const card = page.locator('[data-testid^="live-page-action-"]');

    const launchRow = async (ticket: string) => {
      await frame.locator(`button:has-text("Framer ${ticket}")`).click();
      await expect(card).toBeVisible();
      await expect(card, `${ticket} must open on the offer, not on an earlier launch`)
        .toHaveAttribute('data-state', 'proposed');
      const launched = page.waitForResponse(response =>
        /\/api\/live-page-actions\/[^/]+\/launch$/.test(new URL(response.url()).pathname));
      await card.locator('.discussion-action-card__launch').click();
      const body = (await (await launched).json()) as { data: { id: string } };
      await expect(card).toHaveAttribute('data-state', 'succeeded', { timeout: 10_000 });
      const settled = await page.request.get(`/api/live-page-actions/${body.data.id}`);
      return ((await settled.json()) as { data: { id: string; result_discussion_id: string } }).data;
    };

    const first = await launchRow('EW-7706');
    const second = await launchRow('EW-7704');

    expect(second.id).not.toBe(first.id);
    expect(second.result_discussion_id).not.toBe(first.result_discussion_id);
    const discussion = await page.request.get(`/api/discussions/${second.result_discussion_id}`);
    const content = JSON.stringify(await discussion.json());
    expect(content, 'the second run was framed for its own ticket').toContain('EW-7704');
    expect(content).not.toContain('EW-7706');
  });
});

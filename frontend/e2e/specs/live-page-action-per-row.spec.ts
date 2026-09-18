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
 * The target is a Quick Exec that echoes the ticket it received: it succeeds
 * without an agent, and its output proves in the browser that each row ran
 * with its own value.
 */

import { test, expect } from '../fixtures/kronn-fixture';

test.describe('Live Page inline action — each row is its own launch', () => {
  test('a second ticket launches its own run instead of replaying the first one', async ({ page }) => {
    const qeResponse = await page.request.post('/api/quick-execs', {
      data: {
        name: `E2E frame per row ${Date.now()}`,
        command: 'echo',
        args: ['{"ticket":"{{ticket}}"}'],
        timeout_secs: 10,
        output_format: 'json',
        variables: [{ name: 'ticket', label: 'Ticket', placeholder: '' }],
      },
    });
    expect(qeResponse.ok()).toBe(true);
    const targetId = ((await qeResponse.json()) as { data: { id: string } }).data.id;

    const button = (ticket: string) =>
      `<button data-kronn-action="frame" data-kronn-bindings='{"ticket":"${ticket}"}'>Framer ${ticket}</button>`;
    const html = button('EW-7706') + button('EW-7704')
      + '<script type="application/kronn-action" data-action-id="frame">'
      + `{"kind":"quick_exec","target_id":"${targetId}","values":[{"name":"ticket","provenance":"dynamic_binding","source_ref":"<page.dataset.tickets.find(key).key>"}]}`
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
    const output = page.getByTestId('run-status-card-exec-output');

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
      // The run received this row's value, not the first row's.
      await expect(output).toContainText(ticket);
      const settled = await page.request.get(`/api/live-page-actions/${body.data.id}`);
      return ((await settled.json()) as { data: { id: string; shared_run_id: string } }).data;
    };

    const first = await launchRow('EW-7706');
    const second = await launchRow('EW-7704');
    expect(second.id).not.toBe(first.id);
    expect(second.shared_run_id).not.toBe(first.shared_run_id);

    // Each button in the Page shows how its own row went.
    for (const ticket of ['EW-7706', 'EW-7704']) {
      await expect(frame.locator(`button:has-text("Framer ${ticket}")`))
        .toHaveAttribute('data-kronn-action-state', 'succeeded');
    }
    // Clicking the button of the open card closes it.
    await frame.locator('button:has-text("Framer EW-7704")').click();
    await expect(card).toHaveCount(0);

    // A row that has run reopens on what happened — its own run — with the
    // way to run it again.
    await frame.locator('button:has-text("Framer EW-7706")').click();
    await expect(card).toHaveAttribute('data-state', 'succeeded');
    await expect(output).toContainText('EW-7706');
    await page.getByTestId('action-card-relaunch').click();
    await expect(card).toHaveAttribute('data-state', 'proposed');
  });
});

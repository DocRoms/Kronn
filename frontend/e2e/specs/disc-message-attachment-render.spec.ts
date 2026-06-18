/**
 * Browser E2E — a file pinned to a message renders as an attachment in its
 * bubble (0.8.8, per-message attachments).
 *
 * Before 0.8.8, uploaded images were "sticky" on the whole discussion: they
 * stayed in the composer and never appeared in the sent message. Now the
 * backend pins composer-staged files to the user message at send, and the
 * bubble renders them — image thumbnails fetched as auth'd blobs, filename
 * chips for the rest.
 *
 * This spec is fully stubbed (hermetic + deterministic): it loads a disc whose
 * User message already has one image pinned to it, plus a non-image file, and
 * asserts BOTH render in the bubble. The byte route returns a real 1×1 PNG so
 * the blob → object-URL → <img> path actually executes. The composer stays
 * empty — proving the files moved OUT of the input and INTO the message.
 */
import { test, expect } from '../fixtures/kronn-fixture';
import type { Discussion, DiscussionMessage, ContextFile } from '../../src/types/generated';
import { DashboardPage } from '../pages/DashboardPage';

const DISC_ID = 'e2e-attach-disc';
const MSG_ID = 'm-with-image';
const USER_MSG = 'regarde ce diagramme que je joins';

// Smallest valid PNG (1×1 transparent pixel).
const PNG_1x1_BASE64 =
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==';

function mkMsg(): DiscussionMessage {
  return {
    id: MSG_ID,
    role: 'User',
    content: USER_MSG,
    agent_type: null,
    timestamp: '2026-06-17T10:00:00Z',
    tokens_used: 0,
    auth_mode: null,
    lint_report: null,
  };
}

function mkDisc(): Discussion {
  return {
    id: DISC_ID,
    project_id: null,
    title: 'E2E Attachment Render',
    agent: 'ClaudeCode',
    language: 'fr',
    participants: ['ClaudeCode'],
    messages: [mkMsg()],
    message_count: 1,
    non_system_message_count: 1,
    archived: false,
    pinned: false,
    workspace_mode: 'Direct',
    created_at: '2026-06-17T09:00:00Z',
    updated_at: '2026-06-17T10:00:00Z',
  };
}

function mkFile(over: Partial<ContextFile>): ContextFile {
  return {
    id: 'cf-x',
    discussion_id: DISC_ID,
    filename: 'file',
    mime_type: 'application/octet-stream',
    original_size: 1024,
    extracted_size: 0,
    disk_path: null,
    message_id: MSG_ID,
    created_at: '2026-06-17T10:00:00Z',
    ...over,
  };
}

const envelope = (data: unknown) => JSON.stringify({ success: true, data, error: null });

test.describe('Discussion — a message renders its pinned attachments', () => {
  test('image thumbnail + filename chip both appear in the bubble', async ({ page }) => {
    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope([mkDisc()]) });
    });

    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope(mkDisc()) });
    });

    // Context files for the disc: one image + one text file, both pinned to MSG_ID.
    await page.route(`**/api/discussions/${DISC_ID}/context-files`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([
          mkFile({ id: 'cf-img', filename: 'diagram.png', mime_type: 'image/png', disk_path: '/tmp/diagram.png' }),
          mkFile({ id: 'cf-txt', filename: 'notes.txt', mime_type: 'text/plain', disk_path: null }),
        ]),
      });
    });

    // The thumbnail's auth'd byte fetch → real PNG bytes.
    await page.route(`**/api/discussions/${DISC_ID}/context-files/cf-img/content`, route => {
      return route.fulfill({
        status: 200,
        contentType: 'image/png',
        body: Buffer.from(PNG_1x1_BASE64, 'base64'),
      });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navDiscussions.click();
    await page.getByRole('button', { name: /E2E Attachment Render/ }).click();

    // The message bubble is on screen.
    await expect(
      page.locator('.disc-msg-bubble').filter({ hasText: USER_MSG }),
    ).toBeVisible({ timeout: 10_000 });

    // The image attachment renders as a thumbnail <img> with the fetched blob.
    const thumb = page.locator('.disc-msg-attachments img[alt="diagram.png"]');
    await expect(thumb).toBeVisible({ timeout: 10_000 });
    await expect(thumb).toHaveAttribute('src', /^blob:/);

    // The non-image renders as a filename chip.
    await expect(
      page.locator('.disc-attach-chip').filter({ hasText: 'notes.txt' }),
    ).toBeVisible();

    // And nothing leaked into the composer — the files belong to the message now.
    await expect(page.locator('.disc-context-files')).toHaveCount(0);
  });
});

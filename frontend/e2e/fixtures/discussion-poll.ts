/**
 * An open discussion refreshes through `GET /api/discussions/<id>/poll`, which
 * answers `{ revision, detail }` with `detail: null` when nothing changed since
 * the revision the client sends. A spec that stubs the full discussion must
 * stub this too, or the poll reaches the real backend and the page never sees
 * the stubbed messages change.
 */

import type { Page, Route } from '@playwright/test';

function revisionOf(detail: unknown): string {
  const text = JSON.stringify(detail);
  let hash = 0;
  for (let i = 0; i < text.length; i += 1) hash = (hash * 31 + text.charCodeAt(i)) | 0;
  return `e2e-${(hash >>> 0).toString(36)}-${text.length}`;
}

/** Serve the discussion's poll from `detail()`, the same data the spec gives
 *  its full `GET /api/discussions/<id>` stub. */
export async function stubDiscussionPoll(page: Page, discussionId: string, detail: () => unknown) {
  await page.route(new RegExp(`/api/discussions/${discussionId}/poll(\\?.*)?$`), route => {
    if (route.request().method() !== 'GET') return route.fallback();
    const current = detail();
    const revision = revisionOf(current);
    const known = new URL(route.request().url()).searchParams.get('revision');
    return route.fulfill({
      json: { success: true, data: { revision, detail: known === revision ? null : current }, error: null },
    });
  });
}

/** Same, for a spec whose full-GET stub computes the discussion inline: the
 *  handler runs as if for that GET and its answer becomes the poll's detail. */
export async function stubDiscussionPollFrom(
  page: Page,
  discussionId: string,
  handler: (route: Route) => unknown,
) {
  await page.route(new RegExp(`/api/discussions/${discussionId}/poll(\\?.*)?$`), async route => {
    if (route.request().method() !== 'GET') return route.fallback();
    let body: string | undefined;
    const asFullGet = {
      request: () => ({
        method: () => 'GET',
        url: () => route.request().url().replace(/\/poll(\?.*)?$/, ''),
        headers: () => route.request().headers(),
      }),
      fulfill: async (options: { body?: string; json?: unknown }) => {
        body = options.body ?? JSON.stringify(options.json ?? null);
      },
      continue: async () => undefined,
      fallback: async () => undefined,
    } as unknown as Route;
    await handler(asFullGet);
    const current = body ? (JSON.parse(body) as { data?: unknown }).data ?? null : null;
    const revision = revisionOf(current);
    const known = new URL(route.request().url()).searchParams.get('revision');
    return route.fulfill({
      json: { success: true, data: { revision, detail: known === revision ? null : current }, error: null },
    });
  });
}

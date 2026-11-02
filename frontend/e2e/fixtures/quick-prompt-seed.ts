/**
 * A Quick Prompt the spec owns.
 *
 * Specs that drive the Quick Prompts tab used to read "the first QP card" and
 * skip when the list was empty. That worked only because an earlier spec in
 * the same run had left a Quick Prompt behind: the CI job now runs the suite
 * in three shards (KT-20), so a spec cannot count on what another spec
 * created. Seed in `beforeAll`, delete in `afterAll`.
 */

import { expect } from '@playwright/test';
import type { APIRequestContext } from '@playwright/test';

/** Create a minimal, variable-free Quick Prompt and return its id. */
export async function seedQuickPrompt(request: APIRequestContext, name: string): Promise<string> {
  const response = await request.post('/api/quick-prompts', {
    data: { name, prompt_template: 'Say hello.', variables: [], agent: 'Codex' },
  });
  expect(response.ok(), `POST /api/quick-prompts answered ${response.status()}`).toBe(true);
  const body = await response.json();
  expect(body?.success, `POST /api/quick-prompts: ${JSON.stringify(body?.error)}`).toBe(true);
  return body.data.id as string;
}

/** Best-effort removal: a missing row is not a test failure. */
export async function deleteQuickPrompt(request: APIRequestContext, id: string): Promise<void> {
  if (!id) return;
  await request.delete(`/api/quick-prompts/${id}`).catch(() => { /* already gone */ });
}

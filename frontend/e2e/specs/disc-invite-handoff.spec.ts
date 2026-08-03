/**
 * Invite handoff — real backend, no agent tokens.
 *
 * Exercises the complete handoff boundary: create a discussion, mint the two
 * copy forms, consume the token as a CLI peer, and verify that the joined peer
 * receives the shared-plan/room protocol. The React toggle is covered by the
 * component test; this pins the HTTP contract used by every MCP bridge.
 */

import { test, expect } from '../fixtures/kronn-fixture';
import { DashboardPage } from '../pages/DashboardPage';

const title = `Invite handoff PW ${Date.now()}`;
const taskTitle = `Plan focus PW ${Date.now()}`;
const sessionId = `pw-invite-${Date.now()}`;
let discId: string | null = null;
let taskId: string | null = null;

test.afterAll(async ({ request }) => {
  await request.post('/api/discussions/peer-leave', {
    data: { agent_type: 'ClaudeCode', session_id: sessionId },
  }).catch(() => {});
  if (taskId) {
    await request.patch(`/api/planning/tasks/${taskId}`, {
      data: {
        status: 'archived',
        actor: { kind: 'agent', id: 'Playwright' },
      },
    }).catch(() => {});
  }
  if (discId) {
    await request.delete(`/api/discussions/${discId}`).catch(() => {});
  }
});

test('enriched handoff carries the plan and the task appears in Focus', async ({ request, page }) => {
  const created = await request.post('/api/discussions', {
    data: {
      title,
      agent: 'ClaudeCode',
      language: 'fr',
      initial_prompt: 'Prépare la reprise sans lancer de modèle.',
    },
  });
  expect(created.ok()).toBe(true);
  const createdBody = await created.json();
  expect(createdBody?.success).toBe(true);
  discId = createdBody?.data?.id;
  expect(discId).toBeTruthy();
  if (!discId) throw new Error('discussion creation returned no id');

  const taskCreated = await request.post('/api/planning/tasks', {
    data: {
      title: taskTitle,
      status: 'todo',
      priority: 'high',
      actor: { kind: 'agent', id: 'Playwright' },
    },
  });
  expect(taskCreated.ok()).toBe(true);
  const taskBody = await taskCreated.json();
  expect(taskBody?.success).toBe(true);
  taskId = taskBody?.data?.id;
  expect(taskId).toBeTruthy();

  const linked = await request.post(`/api/planning/tasks/${taskId}/discussions`, {
    data: {
      discussion_id: discId,
      placement: 'active',
      is_primary: true,
      actor: { kind: 'agent', id: 'Playwright' },
    },
  });
  expect(linked.ok()).toBe(true);
  expect((await linked.json())?.success).toBe(true);

  const invited = await request.post(`/api/discussions/${discId}/invite-peer`, { data: {} });
  expect(invited.ok()).toBe(true);
  const inviteBody = await invited.json();
  expect(inviteBody?.success).toBe(true);
  const invite = inviteBody.data;
  expect(invite.instruction_text).toContain('disc_join');
  expect(invite.instruction_text).toContain('plan_get');
  expect(invite.instruction_text).toContain('task_update');
  expect(invite.instruction_text).toContain('disc_wait_for_peer');
  expect(invite.instruction_text_minimal).toContain(invite.token);
  expect(invite.instruction_text_minimal).not.toContain('plan_get');

  const joined = await request.post('/api/discussions/peer-join', {
    data: {
      token: invite.token,
      agent_type: 'ClaudeCode',
      session_id: sessionId,
      model: 'playwright-no-model-run',
    },
  });
  expect(joined.ok()).toBe(true);
  const joinBody = await joined.json();
  expect(joinBody?.success).toBe(true);
  expect(joinBody?.data?.disc_id).toBe(discId);
  expect(joinBody?.data?.next_steps).toContain('plan_get');
  expect(joinBody?.data?.next_steps).toContain('disc_wait_for_peer');
  expect(joinBody?.data?.next_steps).toContain('plan_snapshot');
  expect(joinBody?.data?.plan_snapshot?.primary_objective?.title).toBe(taskTitle);
  expect(joinBody?.data?.plan_snapshot?.current).toEqual(expect.arrayContaining([
    expect.objectContaining({ title: taskTitle, status: 'todo' }),
  ]));

  const dashboard = new DashboardPage(page);
  await dashboard.goto();
  await dashboard.openDiscussion(discId);
  await page.locator('.disc-plan-btn').click();
  await expect(page.locator('.plan-timeline-section[data-kind="upcoming"]')).toContainText(taskTitle);
});

/**
 * Browser E2E — a decision card Kronn's orchestrator posts is visible and
 * answerable (KT-1433).
 *
 * Found live, not by a test: the Docker elevation card Kronn posts in an
 * autoCode room was folded behind the orchestrator's "details" toggle and,
 * once opened, rendered without its message id — "Arbitrage introuvable",
 * and two integrations stayed parked on a card nobody could answer.
 *
 * Hermetic: the room, its questions and the answer endpoint are stubbed. A
 * real backend cannot be made to post an orchestrator card on demand, by
 * design (no route lets a client write one).
 */
import { test, expect } from '../fixtures/kronn-fixture';
import type { Discussion, DiscussionMessage, DiscussionQuestion } from '../../src/types/generated';
import { DashboardPage } from '../pages/DashboardPage';

const DISC_ID = 'e2e-orchestrator-card';
const MSG_ID = 'orch-elevation:docker:e2e-exec:0';
const QUESTION_ID = `question:${MSG_ID}:0`;
const TITLE = 'E2E Orchestrator Card';

const options = [
  { id: 'deny', label: 'Refuser', description: null },
  { id: 'discussion', label: 'Autoriser pour cette discussion', description: null },
  { id: 'project', label: 'Autoriser pour le projet', description: null },
];

function question(answered: boolean): DiscussionQuestion {
  return {
    id: QUESTION_ID, discussion_id: DISC_ID, source_message_id: MSG_ID, fence_index: 0,
    key: 'kronn-elevation-docker', question: "Autoriser Docker pour les validations d'intégration ?",
    context: 'Une validation passe par Docker, que le bac à sable des validations garde fermé.',
    td_errors: null, options, items: [], multiple: false, recommended_option_ids: [], task_ref: null,
    requester_workflow_step: null, resume: null,
    state: answered ? 'answered' : 'pending',
    answer: answered
      ? { selected_option_ids: ['discussion'], item_answers: [], text: null, author_pseudo: 'Romu', answered_at: '2026-10-23T00:00:01Z', message_id: 'answer-msg' }
      : null,
    created_at: '2026-10-23T00:00:00Z', updated_at: '2026-10-23T00:00:00Z',
  } as DiscussionQuestion;
}

function orchestratorMessage(): DiscussionMessage {
  return {
    id: MSG_ID,
    role: 'User',
    content: [
      '**Élévation demandée — Docker**',
      '',
      '```kronn-question',
      JSON.stringify({ version: 1, key: 'kronn-elevation-docker', question: question(false).question, options }),
      '```',
    ].join('\n'),
    agent_type: null,
    timestamp: '2026-10-23T00:00:00Z',
    tokens_used: 0,
    auth_mode: null,
    author_pseudo: 'Orchestrateur',
    kind: 'orchestration',
    lint_report: null,
  } as DiscussionMessage;
}

function room(): Discussion {
  return {
    id: DISC_ID, project_id: null, title: TITLE, agent: 'ClaudeCode', language: 'fr',
    participants: ['ClaudeCode'], messages: [orchestratorMessage()], message_count: 1,
    non_system_message_count: 1, archived: false, pinned: false, workspace_mode: 'Direct',
    created_at: '2026-10-23T00:00:00Z', updated_at: '2026-10-23T00:00:00Z',
  } as Discussion;
}

const envelope = (data: unknown) => JSON.stringify({ success: true, data, error: null });

test.describe('Discussion — an orchestrator decision card', () => {
  test('is shown expanded, bound to its question, and answers the chosen scope', async ({ page }) => {
    let answered = false;
    let answerBody: { selected_option_ids?: string[] } | null = null;

    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope([room()]) });
    });
    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope(room()) });
    });
    await page.route(`**/api/discussions/${DISC_ID}/questions`, route =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope({ questions: [question(answered)], pending_count: answered ? 0 : 1 }),
      }));
    await page.route(`**/api/discussions/${DISC_ID}/questions/*/answer`, route => {
      answerBody = route.request().postDataJSON();
      answered = true;
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope(question(true)) });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.navDiscussions.click();
    await page.locator(`button.disc-item-open[aria-label^="${TITLE} —"]`).click();

    const card = page.getByTestId(`disc-question-${QUESTION_ID}`);
    await expect(card).toBeVisible({ timeout: 10_000 });
    await expect(page.getByText('Arbitrage introuvable')).toHaveCount(0);

    await card.getByTestId('disc-question-option-discussion').click();
    await card.getByTestId('disc-question-send').click();

    await expect.poll(() => answerBody?.selected_option_ids).toEqual(['discussion']);
  });
});

import { test, expect } from '../fixtures/kronn-fixture';
import type {
  AgentDetection,
  Discussion,
  DiscussionMessage,
  SendMessageRequest,
} from '../../src/types/generated';
import { DashboardPage } from '../pages/DashboardPage';

const DISC_ID = 'e2e-multi-model-disc';
const USER_TEXT = 'Vous devriez connaître vos points forts et faibles.';
const LITE_REPLY = 'Réponse LiteLLM terminée';
const OLLAMA_REPLY = 'Réponse Ollama terminée';

const envelope = (data: unknown) => JSON.stringify({ success: true, data, error: null });

function message(
  id: string,
  role: DiscussionMessage['role'],
  content: string,
  agentType: DiscussionMessage['agent_type'] = null,
  replyTo: string | null = null,
): DiscussionMessage {
  return {
    id,
    role,
    channel: 'main',
    content,
    agent_type: agentType,
    timestamp: '2026-08-10T10:01:53Z',
    tokens_used: 0,
    auth_mode: agentType ? 'local' : null,
    lint_report: null,
    reply_to_message_id: replyTo,
  } as DiscussionMessage;
}

function discussion(messages: DiscussionMessage[], awaitingAgent: boolean): Discussion {
  return {
    id: DISC_ID,
    project_id: null,
    title: 'E2E multi-model placeholders',
    agent: 'LiteLlm',
    language: 'fr',
    participants: ['LiteLlm', 'Ollama'],
    messages,
    message_count: messages.length,
    non_system_message_count: messages.length,
    awaiting_agent: awaitingAgent,
    archived: false,
    pinned: false,
    workspace_mode: 'Direct',
    created_at: '2026-08-10T10:00:00Z',
    updated_at: '2026-08-10T10:01:53Z',
  } as Discussion;
}

const agents: AgentDetection[] = [
  {
    name: 'LiteLLM', agent_type: 'LiteLlm', installed: true, enabled: true,
    runtime_available: true, path: null, version: null, latest_version: null,
    origin: 'test', install_command: null, host_managed: false, host_label: null,
    rtk_available: false, rtk_hook_configured: false,
  },
  {
    name: 'Ollama', agent_type: 'Ollama', installed: true, enabled: true,
    runtime_available: true, path: null, version: null, latest_version: null,
    origin: 'test', install_command: null, host_managed: false, host_label: null,
    rtk_available: false, rtk_hook_configured: false,
  },
  {
    name: 'Codex', agent_type: 'Codex', installed: true, enabled: true,
    runtime_available: true, path: null, version: null, latest_version: null,
    origin: 'test', install_command: null, host_managed: false, host_label: null,
    rtk_available: false, rtk_hook_configured: false,
  },
] as AgentDetection[];

test.describe('Discussion chat — multi-model reply lifecycle', () => {
  test('keeps the explicitly requested agent tier visible on the sent message', async ({ page }) => {
    let sentBody: SendMessageRequest | null = null;

    await page.exposeFunction('e2eCaptureTieredSend', (body: SendMessageRequest) => {
      sentBody = body;
    });
    await page.addInitScript(({ discussionId }) => {
      const originalFetch = window.fetch.bind(window);
      window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === 'string'
          ? input
          : input instanceof URL
            ? input.toString()
            : input.url;
        const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
        if (method === 'POST' && url.endsWith(`/api/discussions/${discussionId}/messages`)) {
          const body = JSON.parse(String(init?.body ?? '{}')) as SendMessageRequest;
          await (window as unknown as {
            e2eCaptureTieredSend: (request: SendMessageRequest) => Promise<void>;
          }).e2eCaptureTieredSend(body);
          const encoder = new TextEncoder();
          return new Response(new ReadableStream<Uint8Array>({
            start(controller) {
              controller.enqueue(encoder.encode(
                `event: accepted\ndata: ${JSON.stringify({
                  message_id: body.client_message_id,
                  sort_order: 1,
                  duplicate: false,
                })}\n\n`,
              ));
              controller.enqueue(encoder.encode('event: done\ndata: {}\n\n'));
              controller.close();
            },
          }), {
            status: 200,
            headers: { 'Content-Type': 'text/event-stream' },
          });
        }
        return originalFetch(input, init);
      };
    }, { discussionId: DISC_ID });

    await page.route('**/api/agents', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope(agents) });
    });
    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([discussion([message('seed', 'User', 'Départ')], false)]),
      });
    });
    await page.route(`**/api/discussions/${DISC_ID}/participants`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope([]),
    }));
    await page.route(`**/api/discussions/${DISC_ID}/native-agent`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope({ disabled: false }),
    }));
    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      const sent = sentBody as SendMessageRequest | null;
      const messages = [message('seed', 'User', 'Départ')];
      if (sent?.client_message_id) {
        messages.push(message(sent.client_message_id, 'User', '@codex Analyse rapidement'));
      }
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope({
          ...discussion(messages, false),
          active_agent_dispatches: [],
          message_targets: sent?.client_message_id
            ? { [sent.client_message_id]: sent.targets }
            : {},
        }),
      });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.openDiscussion(DISC_ID);

    const composer = page.locator('.disc-composer-textarea');
    await composer.fill('@cod');
    const codexMention = page.locator('.disc-mention-item').filter({ hasText: '@codex' }).first();
    await codexMention.locator('.disc-mention-tier-choice[data-tier="economy"]').click();
    await composer.fill('@codex Analyse rapidement');
    await page.locator('.disc-send-btn').click();

    await expect.poll(() => sentBody).not.toBeNull();
    expect(sentBody?.targets).toEqual([
      { kind: 'agent', agent_type: 'Codex', cli_session_id: null, tier: 'economy' },
    ]);
    const receipt = page.getByTestId('message-routing-receipt').filter({ hasText: '@codex' });
    await expect(receipt).toContainText('@codex · ⚡ Éco');
  });

  test('an explicit multi-model turn keeps Ollama visible after LiteLLM settles', async ({ page }) => {
    let sentAt = 0;
    let sentBody: SendMessageRequest | null = null;

    await page.exposeFunction('e2eCaptureMultiModelSend', (body: SendMessageRequest) => {
      sentAt = Date.now();
      sentBody = body;
    });
    await page.addInitScript(({ discussionId, liteReply }) => {
      const originalFetch = window.fetch.bind(window);
      window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === 'string'
          ? input
          : input instanceof URL
            ? input.toString()
            : input.url;
        const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
        if (method === 'POST' && url.endsWith(`/api/discussions/${discussionId}/messages`)) {
          const body = JSON.parse(String(init?.body ?? '{}'));
          await (window as unknown as {
            e2eCaptureMultiModelSend: (request: unknown) => Promise<void>;
          }).e2eCaptureMultiModelSend(body);

          const encoder = new TextEncoder();
          const stream = new ReadableStream<Uint8Array>({
            start(controller) {
              controller.enqueue(encoder.encode(
                `event: accepted\ndata: ${JSON.stringify({
                  message_id: body.client_message_id,
                  sort_order: 2,
                  duplicate: false,
                })}\n\n`,
              ));
              window.setTimeout(() => {
                controller.enqueue(encoder.encode(
                  `event: chunk\ndata: ${JSON.stringify({ text: liteReply })}\n\n`,
                ));
              }, 300);
              window.setTimeout(() => {
                controller.enqueue(encoder.encode('event: done\ndata: {}\n\n'));
                controller.close();
              }, 1_200);
            },
          });
          return new Response(stream, {
            status: 200,
            headers: { 'Content-Type': 'text/event-stream' },
          });
        }
        return originalFetch(input, init);
      };
    }, { discussionId: DISC_ID, liteReply: LITE_REPLY });

    await page.route('**/api/agents', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope(agents) });
    });
    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([discussion([message('seed', 'User', 'Départ')], false)]),
      });
    });
    await page.route(`**/api/discussions/${DISC_ID}/participants`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope([]),
    }));
    await page.route(`**/api/discussions/${DISC_ID}/native-agent`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope({ disabled: false }),
    }));
    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      const elapsed = sentAt === 0 ? 0 : Date.now() - sentAt;
      const triggerId = sentBody?.client_message_id ?? null;
      const messages = [message('seed', 'User', 'Départ')];
      if (triggerId) messages.push(message(triggerId, 'User', USER_TEXT));
      if (elapsed >= 1_200) {
        messages.push(message('lite-reply', 'Agent', LITE_REPLY, 'LiteLlm', triggerId));
      }
      if (elapsed >= 5_500) {
        messages.push(message('ollama-reply', 'Agent', OLLAMA_REPLY, 'Ollama', triggerId));
      }
      const activeAgentDispatches = !triggerId || elapsed >= 5_500
        ? []
        : [
            ...(elapsed < 1_200
              ? [{ id: 'lite-job', trigger_message_id: triggerId, agent_type: 'LiteLlm', status: 'Running' }]
              : []),
            { id: 'ollama-job', trigger_message_id: triggerId, agent_type: 'Ollama', status: 'Pending' },
          ];
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope({
          ...discussion(messages, sentAt > 0 && elapsed < 5_500),
          active_agent_dispatches: activeAgentDispatches,
        }),
      });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.openDiscussion(DISC_ID);
    await expect(page.getByText('Départ')).toBeVisible();

    await page.locator('.disc-composer-textarea').fill(`@litellm @ollama ${USER_TEXT}`);
    await page.locator('.disc-send-btn').click();

    await expect(page.getByTestId('streaming-agent-LiteLlm')).toBeVisible();
    await expect(page.getByTestId('pending-agent-Ollama')).toBeVisible();
    await expect.poll(() => sentBody).not.toBeNull();
    expect(sentBody?.targets).toEqual([
      { kind: 'discussion_agent', agent_type: 'LiteLlm', cli_session_id: null, tier: null },
      { kind: 'agent', agent_type: 'Ollama', cli_session_id: null, tier: 'default' },
    ]);

    await expect(page.getByText(LITE_REPLY)).toBeVisible({ timeout: 5_000 });
    await expect(page.getByTestId('streaming-agent-LiteLlm')).toBeHidden();
    await expect(page.getByTestId('pending-agent-Ollama')).toBeVisible();

    await expect(page.getByText(OLLAMA_REPLY)).toBeVisible({ timeout: 12_000 });
    await expect(page.getByTestId('pending-agent-Ollama')).toBeHidden();
  });

  test('overlapping turns keep duplicate model jobs separate and place a late reply before the newer question', async ({ page }) => {
    const oldUserId = 'user-old';
    const newUserText = 'Deuxième tour pendant que le premier Ollama travaille.';
    const oldLateReply = 'Réponse lente du premier tour';
    let secondSentAt = 0;
    let secondBody: SendMessageRequest | null = null;

    await page.exposeFunction('e2eCaptureOverlappingSend', (body: SendMessageRequest) => {
      secondSentAt = Date.now();
      secondBody = body;
    });
    await page.addInitScript(({ discussionId }) => {
      const originalFetch = window.fetch.bind(window);
      window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
        const url = typeof input === 'string'
          ? input
          : input instanceof URL
            ? input.toString()
            : input.url;
        const method = init?.method ?? (input instanceof Request ? input.method : 'GET');
        if (method === 'POST' && url.endsWith(`/api/discussions/${discussionId}/messages`)) {
          const body = JSON.parse(String(init?.body ?? '{}'));
          await (window as unknown as {
            e2eCaptureOverlappingSend: (request: SendMessageRequest) => Promise<void>;
          }).e2eCaptureOverlappingSend(body);
          const encoder = new TextEncoder();
          return new Response(new ReadableStream<Uint8Array>({
            start(controller) {
              controller.enqueue(encoder.encode(
                `event: accepted\ndata: ${JSON.stringify({
                  message_id: body.client_message_id,
                  sort_order: 4,
                  duplicate: false,
                })}\n\n`,
              ));
              controller.enqueue(encoder.encode('event: done\ndata: {}\n\n'));
              controller.close();
            },
          }), { status: 200, headers: { 'Content-Type': 'text/event-stream' } });
        }
        return originalFetch(input, init);
      };
    }, { discussionId: DISC_ID });

    await page.route('**/api/agents', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope(agents) });
    });
    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([{
          ...discussion([
            message(oldUserId, 'User', 'Premier tour'),
            message('old-lite', 'Agent', 'Lite du premier tour', 'LiteLlm', oldUserId),
          ], true),
          participants: ['LiteLlm', 'Ollama', 'Codex'],
        }]),
      });
    });
    await page.route(`**/api/discussions/${DISC_ID}/participants`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope([]),
    }));
    await page.route(`**/api/discussions/${DISC_ID}/native-agent`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope({ disabled: false }),
    }));
    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      const elapsed = secondSentAt === 0 ? 0 : Date.now() - secondSentAt;
      const messages = [
        message(oldUserId, 'User', 'Premier tour'),
        message('old-lite', 'Agent', 'Lite du premier tour', 'LiteLlm', oldUserId),
      ];
      if (secondSentAt > 0) messages.push(message('user-new', 'User', newUserText));
      if (elapsed >= 1_200) {
        messages.push(message('old-ollama', 'Agent', oldLateReply, 'Ollama', oldUserId));
      }
      const active = secondSentAt === 0
        ? [{ id: 'old-ollama-job', trigger_message_id: oldUserId, agent_type: 'Ollama', status: 'Running' }]
        : [
            ...(elapsed < 1_200
              ? [{ id: 'old-ollama-job', trigger_message_id: oldUserId, agent_type: 'Ollama', status: 'Running' }]
              : []),
            { id: 'new-lite-job', trigger_message_id: 'user-new', agent_type: 'LiteLlm', status: 'Pending' },
            { id: 'new-ollama-job', trigger_message_id: 'user-new', agent_type: 'Ollama', status: 'Pending' },
            { id: 'new-codex-job', trigger_message_id: 'user-new', agent_type: 'Codex', status: 'Pending' },
          ];
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope({
          ...discussion(messages, true),
          participants: ['LiteLlm', 'Ollama', 'Codex'],
          active_agent_dispatches: active,
        }),
      });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.openDiscussion(DISC_ID);
    await expect(page.locator('[data-testid="pending-agent-Ollama"][data-reply-trigger="user-old"]'))
      .toBeVisible();

    await page.locator('.disc-composer-textarea')
      .fill(`@litellm @ollama @codex ${newUserText}`);
    await page.locator('.disc-send-btn').click();
    await expect.poll(() => secondBody).not.toBeNull();
    expect(secondBody?.targets).toEqual([
      { kind: 'discussion_agent', agent_type: 'LiteLlm', cli_session_id: null, tier: null },
      { kind: 'agent', agent_type: 'Ollama', cli_session_id: null, tier: 'default' },
      { kind: 'agent', agent_type: 'Codex', cli_session_id: null, tier: 'default' },
    ]);

    await expect(page.locator('[data-testid="pending-agent-Ollama"]')).toHaveCount(2);
    const oldPlaceholder = page.locator('[data-reply-trigger="user-old"]');
    const newQuestion = page.getByText(newUserText);
    expect(await oldPlaceholder.evaluate((oldNode, newNode) => (
      Boolean(oldNode.compareDocumentPosition(newNode as Node) & Node.DOCUMENT_POSITION_FOLLOWING)
    ), await newQuestion.elementHandle())).toBe(true);

    await expect(page.getByText(oldLateReply)).toBeVisible({ timeout: 8_000 });
    expect(await page.getByText(oldLateReply).evaluate((replyNode, questionNode) => (
      Boolean(replyNode.compareDocumentPosition(questionNode as Node) & Node.DOCUMENT_POSITION_FOLLOWING)
    ), await newQuestion.elementHandle())).toBe(true);
    await expect(page.locator('[data-testid="pending-agent-Ollama"]')).toHaveCount(1);
  });

  test('an agent-triggered handoff keeps its placeholder and late reply inside the source turn', async ({ page }) => {
    const rootUserId = 'handoff-root-user';
    const sourceAgentId = 'handoff-source-agent';
    const laterQuestion = 'Question humaine envoyée après la délégation.';
    const handoffReply = 'Réponse Ollama au relais explicite.';
    let detailLoadedAt = 0;

    await page.route('**/api/agents', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope(agents) });
    });
    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([discussion([
          message(rootUserId, 'User', 'Demande initiale'),
          message(sourceAgentId, 'Agent', '@Ollama, vérifie ce point.', 'LiteLlm', rootUserId),
          message('handoff-later-user', 'User', laterQuestion),
        ], true)]),
      });
    });
    await page.route(`**/api/discussions/${DISC_ID}/participants`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope([]),
    }));
    await page.route(`**/api/discussions/${DISC_ID}/native-agent`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope({ disabled: false }),
    }));
    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      if (detailLoadedAt === 0) detailLoadedAt = Date.now();
      const settled = Date.now() - detailLoadedAt >= 1_800;
      const messages = [
        message(rootUserId, 'User', 'Demande initiale'),
        message(sourceAgentId, 'Agent', '@Ollama, vérifie ce point.', 'LiteLlm', rootUserId),
        message('handoff-later-user', 'User', laterQuestion),
      ];
      if (settled) {
        messages.push(message('handoff-child-agent', 'Agent', handoffReply, 'Ollama', sourceAgentId));
      }
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope({
          ...discussion(messages, !settled),
          active_agent_dispatches: settled ? [] : [{
            id: 'handoff-child-job',
            trigger_message_id: sourceAgentId,
            agent_type: 'Ollama',
            status: 'Running',
          }],
        }),
      });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.openDiscussion(DISC_ID);
    const placeholder = page.locator(
      `[data-testid="pending-agent-Ollama"][data-reply-trigger="${sourceAgentId}"]`,
    );
    const laterQuestionNode = page.getByText(laterQuestion);
    await expect(placeholder).toBeVisible();
    expect(await placeholder.evaluate((node, laterNode) => (
      Boolean(node.compareDocumentPosition(laterNode as Node) & Node.DOCUMENT_POSITION_FOLLOWING)
    ), await laterQuestionNode.elementHandle())).toBe(true);

    await expect(page.getByText(handoffReply)).toBeVisible({ timeout: 8_000 });
    expect(await page.getByText(handoffReply).evaluate((node, laterNode) => (
      Boolean(node.compareDocumentPosition(laterNode as Node) & Node.DOCUMENT_POSITION_FOLLOWING)
    ), await laterQuestionNode.elementHandle())).toBe(true);
    await expect(placeholder).toBeHidden();
  });

  test('an unavailable LiteLLM target exposes one targeted retry without replaying siblings', async ({ page }) => {
    const failedDispatchId = 'failed-lite-dispatch';
    let retried = false;
    let retryBody: { dispatch_id?: string; idempotency_key?: string } | null = null;
    const errorContent = () => '[kronn:agent-error]\n' + JSON.stringify({
      kind: 'agent_error',
      status: null,
      summary: 'LiteLLM est temporairement indisponible.',
      detail: 'LiteLLM unreachable: VPN unavailable',
      tier: 'default',
      retry_dispatch_id: failedDispatchId,
      retried,
    });

    await page.route('**/api/agents', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({ status: 200, contentType: 'application/json', body: envelope(agents) });
    });
    await page.route('**/api/discussions', route => {
      if (route.request().method() !== 'GET') return route.continue();
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope([discussion([message('seed', 'User', USER_TEXT)], false)]),
      });
    });
    await page.route(`**/api/discussions/${DISC_ID}/participants`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope([]),
    }));
    await page.route(`**/api/discussions/${DISC_ID}/native-agent`, route => route.fulfill({
      status: 200, contentType: 'application/json', body: envelope({ disabled: false }),
    }));
    await page.route(`**/api/discussions/${DISC_ID}/agent-dispatches/retry`, async route => {
      retryBody = route.request().postDataJSON() as typeof retryBody;
      retried = true;
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope({
          dispatch_id: 'retry-lite-dispatch',
          trigger_message_id: 'seed',
          agent_type: 'LiteLlm',
          duplicate: false,
        }),
      });
    });
    await page.route(`**/api/discussions/${DISC_ID}`, route => {
      if (route.request().method() !== 'GET') return route.continue();
      const failed = message('lite-error', 'System', errorContent(), 'LiteLlm', 'seed');
      failed.model = 'proxy-model';
      failed.model_tier = 'default';
      return route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: envelope({
          ...discussion([message('seed', 'User', USER_TEXT), failed], retried),
          active_agent_dispatches: retried ? [{
            id: 'retry-lite-dispatch',
            trigger_message_id: 'seed',
            agent_type: 'LiteLlm',
            status: 'Pending',
          }] : [],
        }),
      });
    });

    const dashboard = new DashboardPage(page);
    await dashboard.goto();
    await dashboard.openDiscussion(DISC_ID);

    await expect(page.getByTestId('disc-model-error-content'))
      .toContainText('LiteLLM est temporairement indisponible.');
    await page.getByTestId('retry-agent-dispatch').click();

    await expect.poll(() => retryBody?.dispatch_id ?? null).toBe(failedDispatchId);
    expect(retryBody?.idempotency_key).toMatch(/^[0-9a-f-]{36}$/);
    await expect(page.getByTestId('retry-agent-dispatch')).toBeHidden();
  });
});

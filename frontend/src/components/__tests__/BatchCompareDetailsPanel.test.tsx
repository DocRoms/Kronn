import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { BatchCompareDetailsPanel } from '../BatchCompareDetailsPanel';
import type { Discussion, MessageTarget, ModelTiersConfig } from '../../types/generated';
import type { ExternalApiConnectionView } from '../../lib/api';

const compareApi = vi.hoisted(() => ({
  get: vi.fn(),
}));

vi.mock(import('../../lib/api'), async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    workflows: {
      ...actual.workflows,
      getBatchCompareDetails: compareApi.get,
    },
  };
});

const zeroModelTiers: ModelTiersConfig = {
  claude_code: { economy: null, default: null, reasoning: null },
  codex: { economy: null, default: null, reasoning: null },
  open_code: { economy: null, default: null, reasoning: null },
  gemini_cli: { economy: null, default: null, reasoning: null },
  kiro: { economy: null, default: null, reasoning: null },
  vibe: { economy: null, default: null, reasoning: null },
  copilot_cli: { economy: null, default: null, reasoning: null },
  ollama: { economy: null, default: null, reasoning: null },
  lite_llm: { economy: null, default: null, reasoning: null },
  nvidia: { economy: null, default: null, reasoning: null },
};

function customConnection(
  overrides: Partial<ExternalApiConnectionView> & { id: string },
): ExternalApiConnectionView {
  return {
    display_name: 'Gateway',
    mention_alias: overrides.id,
    endpoint: 'https://gateway.example/v1',
    credential_slug: overrides.id,
    origin_preset: 'other',
    economy_model: null,
    default_model: null,
    reasoning_model: null,
    created_at: '2026-09-01T00:00:00Z',
    updated_at: '2026-09-01T00:00:00Z',
    has_credential: true,
    ...overrides,
  };
}

function discussion(id: string, agent: Discussion['agent'], tier: Discussion['tier']): Discussion {
  return {
    id,
    title: `Result ${id}`,
    agent,
    tier,
    awaiting_agent: false,
    messages: [],
  } as unknown as Discussion;
}

const t = (key: string, ...args: (string | number)[]) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`;

describe('BatchCompareDetailsPanel — model identity in the ranking list', () => {
  it('never guesses a missing historical model from a tier configuration that changed after the run', async () => {
    compareApi.get.mockResolvedValue({
      run_id: 'run-config-drifted',
      prompt_compatibility: 'same',
      improvement_availability: 'available',
      latest_judge_run: null,
      evaluations: [],
    });
    const noHistory = discussion('disc-no-history', 'Codex', 'reasoning');

    render(
      <BatchCompareDetailsPanel
        runId="run-config-drifted"
        discussions={[noHistory]}
        availableAgents={['Codex']}
        modelTiers={{
          ...zeroModelTiers,
          codex: { economy: null, default: null, reasoning: 'codex-reasoning-v9-today' },
        }}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={t}
      />,
    );

    await screen.findByLabelText('disc.compare.rankBy');
    expect(screen.queryByText('codex-reasoning-v9-today')).not.toBeInTheDocument();
    expect(screen.getByText('disc.defaultAgentModel')).toBeInTheDocument();
  });

  it('keeps a discussion with no response an honest unknown instead of a guessed model', async () => {
    compareApi.get.mockResolvedValue({
      run_id: 'run-no-response',
      prompt_compatibility: 'same',
      improvement_availability: 'available',
      latest_judge_run: null,
      evaluations: [],
    });
    const failed = discussion('disc-no-response', 'ClaudeCode', 'reasoning');
    failed.messages = [{
      id: 'system-no-response',
      role: 'System',
      channel: 'main',
      content: 'Configuration required: authentication is not ready.',
      timestamp: '2026-09-01T10:00:00Z',
      tokens_used: 0,
    }] as Discussion['messages'];

    render(
      <BatchCompareDetailsPanel
        runId="run-no-response"
        discussions={[failed]}
        availableAgents={['ClaudeCode']}
        modelTiers={{
          ...zeroModelTiers,
          claude_code: { economy: null, default: null, reasoning: 'claude-reasoning-today' },
        }}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={t}
      />,
    );

    await screen.findByLabelText('disc.compare.rankBy');
    expect(screen.queryByText('claude-reasoning-today')).not.toBeInTheDocument();
    expect(screen.getByText('disc.defaultAgentModel')).toBeInTheDocument();
    expect(screen.getByText(/authentication is not ready/)).toBeInTheDocument();
  });

  it('keeps each homonymous connection its own recorded model in the ranking card', async () => {
    compareApi.get.mockResolvedValue({
      run_id: 'run-homonyms',
      prompt_compatibility: 'same',
      improvement_availability: 'available',
      latest_judge_run: null,
      evaluations: [],
    });
    const fromA = discussion('disc-gateway-a', 'Custom', 'default');
    fromA.messages = [{
      id: 'm-disc-gateway-a', role: 'Agent', channel: 'main', content: 'Answer from A',
      agent_type: 'Custom', timestamp: '2026-09-01T10:00:00Z', tokens_used: 10, duration_ms: 100,
      model: 'model-from-a',
    }] as Discussion['messages'];
    (fromA as Discussion & { message_targets: Record<string, MessageTarget[]> }).message_targets = {
      'm-disc-gateway-a': [{
        kind: 'discussion_agent', agent_type: 'Custom', connection_id: 'conn-gateway-a', tier: 'default',
      }],
    };
    const fromB = discussion('disc-gateway-b', 'Custom', 'default');
    fromB.messages = [{
      id: 'm-disc-gateway-b', role: 'Agent', channel: 'main', content: 'Answer from B',
      agent_type: 'Custom', timestamp: '2026-09-01T10:00:00Z', tokens_used: 10, duration_ms: 100,
      model: 'model-from-b',
    }] as Discussion['messages'];
    (fromB as Discussion & { message_targets: Record<string, MessageTarget[]> }).message_targets = {
      'm-disc-gateway-b': [{
        kind: 'discussion_agent', agent_type: 'Custom', connection_id: 'conn-gateway-b', tier: 'default',
      }],
    };

    render(
      <BatchCompareDetailsPanel
        runId="run-homonyms"
        discussions={[fromA, fromB]}
        availableAgents={['Custom']}
        externalConnections={[
          customConnection({ id: 'conn-gateway-a', default_model: 'a-default' }),
          customConnection({ id: 'conn-gateway-b', default_model: 'b-default' }),
        ]}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={t}
      />,
    );

    await screen.findByLabelText('disc.compare.rankBy');
    expect(screen.getAllByText('Gateway')).toHaveLength(2);
    const models = Array.from(document.querySelectorAll('.disc-compare-card-model span'))
      .map(node => node.textContent);
    expect(models).toEqual(['model-from-a', 'model-from-b']);
  });

  it('prefers the response-attested model over an older recorded one, with no inferred badge', async () => {
    compareApi.get.mockResolvedValue({
      run_id: 'run-attested',
      prompt_compatibility: 'same',
      improvement_availability: 'available',
      latest_judge_run: null,
      evaluations: [],
    });
    const withHistory = discussion('disc-attested', 'ClaudeCode', 'default');
    withHistory.messages = [
      {
        id: 'm-older', role: 'Agent', channel: 'main', content: 'Older answer',
        agent_type: 'ClaudeCode', timestamp: '2026-09-01T09:00:00Z', tokens_used: 5, duration_ms: 50,
        model: 'older-recorded-model',
      },
      {
        id: 'm-final', role: 'Agent', channel: 'main', content: 'Final answer',
        agent_type: 'ClaudeCode', timestamp: '2026-09-01T10:00:00Z', tokens_used: 10, duration_ms: 100,
        model: 'final-attested-model',
      },
    ] as Discussion['messages'];

    render(
      <BatchCompareDetailsPanel
        runId="run-attested"
        discussions={[withHistory]}
        availableAgents={['ClaudeCode']}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={t}
      />,
    );

    await screen.findByLabelText('disc.compare.rankBy');
    expect(screen.getByText('final-attested-model')).toBeInTheDocument();
    expect(screen.queryByText('older-recorded-model')).not.toBeInTheDocument();
    expect(screen.queryByText('disc.compare.modelRecorded')).not.toBeInTheDocument();
    expect(screen.queryByText('disc.compare.modelUnknown')).not.toBeInTheDocument();
  });

  it('falls back to an older recorded model when the final answer has no model, never the live discussion override', async () => {
    compareApi.get.mockResolvedValue({
      run_id: 'run-missing-final-model',
      prompt_compatibility: 'same',
      improvement_availability: 'available',
      latest_judge_run: null,
      evaluations: [],
    });
    const missingFinalModel = discussion('disc-missing-final-model', 'ClaudeCode', 'default');
    missingFinalModel.model = 'live-discussion-override';
    missingFinalModel.messages = [
      {
        id: 'm-older', role: 'Agent', channel: 'main', content: 'Older answer',
        agent_type: 'ClaudeCode', timestamp: '2026-09-01T09:00:00Z', tokens_used: 5, duration_ms: 50,
        model: 'older-recorded-model',
      },
      {
        id: 'm-final', role: 'Agent', channel: 'main', content: 'Final answer, legacy row',
        agent_type: 'ClaudeCode', timestamp: '2026-09-01T10:00:00Z', tokens_used: 10, duration_ms: 100,
        model: '   ',
      },
    ] as Discussion['messages'];

    render(
      <BatchCompareDetailsPanel
        runId="run-missing-final-model"
        discussions={[missingFinalModel]}
        availableAgents={['ClaudeCode']}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={t}
      />,
    );

    await screen.findByLabelText('disc.compare.rankBy');
    expect(screen.getByText('older-recorded-model')).toBeInTheDocument();
    expect(screen.queryByText('live-discussion-override')).not.toBeInTheDocument();
    expect(screen.getByText('disc.compare.modelRecorded')).toBeInTheDocument();
  });

  it('never labels the live discussion override as the model behind a run with no response at all', async () => {
    compareApi.get.mockResolvedValue({
      run_id: 'run-no-response-override',
      prompt_compatibility: 'same',
      improvement_availability: 'available',
      latest_judge_run: null,
      evaluations: [],
    });
    const noResponse = discussion('disc-no-response-override', 'ClaudeCode', 'default');
    noResponse.model = 'live-discussion-override';
    noResponse.messages = [{
      id: 'system-only', role: 'System', channel: 'main',
      content: 'Configuration required: authentication is not ready.',
      timestamp: '2026-09-01T10:00:00Z', tokens_used: 0,
    }] as Discussion['messages'];

    render(
      <BatchCompareDetailsPanel
        runId="run-no-response-override"
        discussions={[noResponse]}
        availableAgents={['ClaudeCode']}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={t}
      />,
    );

    await screen.findByLabelText('disc.compare.rankBy');
    expect(screen.queryByText('live-discussion-override')).not.toBeInTheDocument();
    expect(screen.getByText('disc.defaultAgentModel')).toBeInTheDocument();
    expect(screen.getByText('disc.compare.modelUnknown')).toBeInTheDocument();
  });
});

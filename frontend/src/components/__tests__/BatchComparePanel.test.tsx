import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { BatchComparePanel } from '../BatchComparePanel';
import type { Discussion, MessageTarget, ModelTiersConfig } from '../../types/generated';
import type { ExternalApiConnectionView } from '../../lib/api';

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

const compareApi = vi.hoisted(() => ({
  get: vi.fn(),
  updateManual: vi.fn(),
  startJudge: vi.fn(),
  startImprovement: vi.fn(),
}));

vi.mock(import('../../lib/api'), async (importOriginal) => {
  const actual = await importOriginal();
  return {
    ...actual,
    workflows: {
      ...actual.workflows,
      getBatchCompareDetails: compareApi.get,
      updateBatchCompareManualScore: compareApi.updateManual,
      startBatchCompareJudge: compareApi.startJudge,
      startBatchCompareImprovement: compareApi.startImprovement,
    },
  };
});

function discussion(id: string, agent: Discussion['agent'], tier: Discussion['tier'], answer: string): Discussion {
  return {
    id,
    title: `Result ${id}`,
    agent,
    tier,
    awaiting_agent: false,
    messages: [{
      id: `m-${id}`,
      role: 'Agent',
      channel: 'main',
      content: answer,
      agent_type: agent,
      timestamp: '2026-08-16T10:00:00Z',
      tokens_used: 120,
      duration_ms: 2300,
      model: `${agent}-model`,
    }],
  } as Discussion;
}

describe('BatchComparePanel', () => {
  it('renders two rich answers side-by-side and keeps each discussion openable', () => {
    const onOpenDiscussion = vi.fn();
    const t = (key: string, ...args: (string | number)[]) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`;
    render(
      <BatchComparePanel
        runId="run-1"
        label="Compare Jira"
        discussions={[
          discussion('disc-codex', 'Codex', 'default', '## Codex answer\n\n**Actionable**'),
          discussion('disc-claude', 'ClaudeCode', 'reasoning', '## Claude answer\n\n- Alternative'),
        ]}
        loading={false}
        error={null}
        availableAgents={['Codex', 'ClaudeCode']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={onOpenDiscussion}
        onClose={vi.fn()}
        t={t}
      />,
    );

    const columns = document.querySelector('.disc-compare-columns');
    expect(columns).toHaveAttribute('data-layout', 'split');
    expect(screen.getByRole('heading', { name: 'Codex answer' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Claude answer' })).toBeInTheDocument();
    expect(screen.getByText('Actionable')).toBeInTheDocument();
    expect(screen.getAllByText('2.3 s')).toHaveLength(2);
    expect(screen.getAllByText('120')).toHaveLength(2);

    const openButtons = screen.getAllByRole('button', { name: /disc.compare.openDiscussion/ });
    fireEvent.click(openButtons[1]);
    expect(onOpenDiscussion).toHaveBeenCalledWith('disc-claude');
  });

  it('moves columns left and right while keeping their answers attached', () => {
    const rendered = render(
      <BatchComparePanel
        runId="run-1"
        label="Reorder compare"
        discussions={[
          discussion('disc-codex', 'Codex', 'default', 'Codex body'),
          discussion('disc-claude', 'ClaudeCode', 'default', 'Claude body'),
          discussion('disc-vibe', 'Vibe', 'default', 'Vibe body'),
        ]}
        loading={false}
        error={null}
        availableAgents={['Codex', 'ClaudeCode', 'Vibe']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key, ...args) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`}
      />,
    );

    const agentOrder = () => Array.from(document.querySelectorAll('.disc-compare-column'))
      .map(column => column.querySelector('.disc-compare-target-name strong')?.textContent);
    expect(agentOrder()).toEqual(['Codex', 'Claude Code', 'Vibe']);

    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.moveRight Codex' }));
    expect(agentOrder()).toEqual(['Claude Code', 'Codex', 'Vibe']);
    expect(document.querySelectorAll('.disc-compare-answer')[1]).toHaveTextContent('Codex body');

    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.moveLeft Vibe' }));
    expect(agentOrder()).toEqual(['Claude Code', 'Vibe', 'Codex']);

    // A live refresh replaces every Discussion object and may append a child.
    // User-controlled ordering must survive without copying props into state.
    rendered.rerender(
      <BatchComparePanel
        runId="run-1"
        label="Reorder compare"
        discussions={[
          discussion('disc-codex', 'Codex', 'default', 'Codex refreshed'),
          discussion('disc-claude', 'ClaudeCode', 'default', 'Claude refreshed'),
          discussion('disc-vibe', 'Vibe', 'default', 'Vibe refreshed'),
          discussion('disc-gemini', 'GeminiCli', 'default', 'Gemini new'),
        ]}
        loading={false}
        error={null}
        availableAgents={['Codex', 'ClaudeCode', 'Vibe', 'GeminiCli']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key, ...args) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`}
      />,
    );
    expect(agentOrder()).toEqual(['Claude Code', 'Vibe', 'Codex', 'Gemini CLI']);
    expect(document.querySelectorAll('.disc-compare-answer')[0]).toHaveTextContent('Claude refreshed');
  });

  it('shows a live generating state while a child is still running', () => {
    const pending = discussion('disc-running', 'Codex', 'default', '');
    pending.messages = pending.messages.filter(message => message.role !== 'Agent');
    pending.awaiting_agent = true;
    render(
      <BatchComparePanel
        runId="run-1"
        label="Live compare"
        discussions={[pending]}
        loading={false}
        error={null}
        availableAgents={['Codex']}
        runningIds={new Set(['disc-running'])}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key, ...args) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`}
      />,
    );

    expect(screen.getByText('disc.compare.generating')).toBeInTheDocument();
    expect(document.querySelector('.disc-compare-column')).toHaveAttribute('data-running', 'true');
  });

  it('surfaces the terminal system cause when an agent never produced an answer', () => {
    const failed = discussion('disc-vibe-failed', 'Vibe', 'default', '');
    failed.messages = [{
      id: 'system-vibe-failed',
      role: 'System',
      channel: 'main',
      content: 'Configuration required: Vibe authentication is not ready.',
      timestamp: '2026-08-16T10:00:00Z',
      tokens_used: 0,
    }] as Discussion['messages'];
    render(
      <BatchComparePanel
        runId="run-1"
        label="Failed compare"
        discussions={[failed]}
        loading={false}
        error={null}
        availableAgents={['Vibe']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.getByRole('alert')).toHaveTextContent('disc.compare.failureReason');
    expect(screen.getByRole('alert')).toHaveTextContent('Vibe authentication is not ready');
    expect(screen.queryByText('disc.compare.noAnswer')).not.toBeInTheDocument();
  });

  it('identifies a failed OpenRouter target and lets the terminal error override a stale spinner', () => {
    const failed = discussion('disc-openrouter-failed', 'Custom', 'default', '');
    failed.title = 'An editable title with no provider identity';
    failed.awaiting_agent = false;
    failed.messages = [{
      id: 'system-openrouter-failed',
      role: 'System',
      channel: 'main',
      content: 'External API error 402 Payment Required: insufficient credits.',
      agent_type: 'Custom',
      model: 'z-ai/glm-5.3',
      timestamp: '2026-08-30T13:50:25Z',
      tokens_used: 0,
    }] as Discussion['messages'];
    (failed as Discussion & { message_targets: Record<string, MessageTarget[]> }).message_targets = {
      'initial-user-message': [{
        kind: 'discussion_agent',
        agent_type: 'Custom',
        connection_id: 'conn-openrouter',
        tier: 'default',
      }],
    };
    const openRouterConnection: ExternalApiConnectionView = {
      id: 'conn-openrouter',
      display_name: 'OpenRouter',
      mention_alias: 'openrouter',
      endpoint: 'https://openrouter.ai/api/v1',
      credential_slug: 'openrouter',
      origin_preset: 'open_router',
      economy_model: 'qwen/qwen3.8-flash',
      default_model: 'z-ai/glm-5.3',
      reasoning_model: 'z-ai/glm-5.3',
      created_at: '2026-08-30T12:00:00Z',
      updated_at: '2026-08-30T12:00:00Z',
      has_credential: true,
    };

    render(
      <BatchComparePanel
        runId="run-openrouter"
        label="Translate article"
        discussions={[failed]}
        loading={false}
        error={null}
        availableAgents={['Custom']}
        externalConnections={[openRouterConnection]}
        runningIds={new Set(['disc-openrouter-failed'])}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.getByText('OpenRouter')).toBeInTheDocument();
    expect(screen.getByText('z-ai/glm-5.3')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('402 Payment Required');
    expect(screen.queryByText('disc.compare.generating')).not.toBeInTheDocument();
    expect(document.querySelector('.disc-compare-column')).toHaveAttribute('data-running', 'false');
  });

  it('opens Details, ranks by every selectable metric and persists a separate human score', async () => {
    const details = {
      run_id: 'run-1',
      latest_judge_run: {
        id: 'judge-1', status: 'Completed', judge_agent: 'Ollama', judge_tier: 'reasoning',
        self_evaluation: false, judge_model: 'qwen', judge_discussion_id: 'judge-disc',
        rubric_version: 'compare-quality-v2', error: null, tokens_used: 80, duration_ms: 900,
        started_at: '2026-08-20T20:00:00Z', finished_at: '2026-08-20T20:00:01Z',
        prompt_review: {
          worth_improving: true,
          strengths: ['Clear goal'],
          weaknesses: [{ text: 'Loose format', affects: 'all' }],
          recommendations: [{ text: 'Pin the schema', affects: 'some' }],
        },
      },
      evaluations: [
        {
          discussion_id: 'disc-codex',
          manual_score: 3,
          manual_updated_at: null,
          ai: {
            judge_run_id: 'judge-1', score: 4, confidence: 0.8,
            positives: ['Precise'], negatives: ['Brief'], contract_violations: [],
          },
        },
        { discussion_id: 'disc-claude', manual_score: null, manual_updated_at: null, ai: null },
      ],
    };
    compareApi.get.mockResolvedValue(details);
    compareApi.updateManual.mockResolvedValue({
      ...details,
      evaluations: [
        { discussion_id: 'disc-codex', manual_score: 4, manual_updated_at: '2026-08-20T20:00:00Z', ai: null },
        details.evaluations[1],
      ],
    });
    compareApi.startImprovement.mockResolvedValue({ discussion_id: 'disc-improvement' });
    const onOpenDiscussion = vi.fn();
    render(
      <BatchComparePanel
        runId="run-1"
        label="Rank compare"
        discussions={[
          discussion('disc-codex', 'Codex', 'default', 'Codex body'),
          discussion('disc-claude', 'ClaudeCode', 'default', 'Claude body'),
        ]}
        loading={false}
        error={null}
        availableAgents={['Codex', 'ClaudeCode']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={onOpenDiscussion}
        onClose={vi.fn()}
        t={(key, ...args) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.details' }));
    expect(await screen.findByRole('heading', { name: 'disc.compare.detailsTitle' })).toBeInTheDocument();
    expect(compareApi.get).toHaveBeenCalledWith('run-1');
    // The details shell renders before compareApi.get() has populated the judge.
    // Await the data-dependent warning so a busy runner cannot race this assertion.
    expect(await screen.findByText('disc.compare.cliJudgeWarning')).toBeInTheDocument();

    const selector = screen.getByLabelText('disc.compare.rankBy');
    expect(Array.from((selector as HTMLSelectElement).options).map(option => option.value))
      .toEqual(['weighted', 'ai', 'human', 'duration', 'tokens']);
    fireEvent.change(selector, { target: { value: 'tokens' } });
    expect(screen.getByRole('button', { name: 'disc.compare.rankDirection' })).toHaveTextContent('disc.compare.ascending');
    expect(screen.getAllByText('disc.compare.rankWeighted')).toHaveLength(3); // selector + 2 cards
    expect(screen.getAllByText('disc.compare.rankAi')).toHaveLength(3);
    expect(screen.getAllByText('disc.compare.rankHuman')).toHaveLength(3);
    expect(screen.getAllByText('disc.compare.rankDuration')).toHaveLength(3);
    expect(screen.getAllByText('disc.compare.rankTokens')).toHaveLength(3);
    expect(screen.getAllByText('Codex-model')).toHaveLength(2); // column header + Details card
    expect(screen.getAllByText('ClaudeCode-model')).toHaveLength(2);
    const aiDetails = screen.getByText('disc.compare.aiFeedback').closest('details');
    expect(aiDetails).not.toHaveAttribute('open');
    fireEvent.click(screen.getByText('disc.compare.aiFeedback'));
    expect(aiDetails).toHaveAttribute('open');
    fireEvent.click(screen.getByText('disc.compare.aiFeedback'));
    expect(aiDetails).not.toHaveAttribute('open');
    const promptReview = screen.getByText('disc.compare.promptReview').closest('details');
    expect(promptReview).not.toHaveAttribute('open');
    fireEvent.click(screen.getByText('disc.compare.promptReview'));
    expect(promptReview).toHaveAttribute('open');
    expect(screen.getByText('disc.compare.affectsAll')).toBeInTheDocument();
    expect(screen.getByText('disc.compare.affectsSome')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.setHumanScore 4 Codex' }));
    await waitFor(() => expect(compareApi.updateManual).toHaveBeenCalledWith('run-1', 'disc-codex', 4));

    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.improvePrompt' }));
    await waitFor(() => expect(compareApi.startImprovement).toHaveBeenCalledWith('run-1', {
      agent: 'Codex',
      tier: 'reasoning',
      connection_id: null,
    }));
    expect(onOpenDiscussion).toHaveBeenCalledWith('disc-improvement');
  });

  it('selects usable OpenCode as the compare judge and sends its native identity', async () => {
    compareApi.get.mockResolvedValue({
      run_id: 'run-opencode-judge',
      prompt_compatibility: 'same',
      improvement_availability: 'available',
      latest_judge_run: null,
      evaluations: [],
    });
    compareApi.startJudge.mockResolvedValue({});
    render(
      <BatchComparePanel
        runId="run-opencode-judge"
        label="OpenCode judge"
        discussions={[discussion('disc-opencode', 'OpenCode', 'default', 'OpenCode answer')]}
        loading={false}
        error={null}
        availableAgents={['Codex', 'OpenCode']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key, ...args) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.details' }));
    await screen.findByText('disc.compare.aiJudge');
    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.chooseJudge' }));
    fireEvent.click(await screen.findByRole('menuitem', { name: 'OpenCode · default' }));
    expect(screen.getByRole('button', { name: 'disc.compare.chooseJudge' })).toHaveTextContent('OpenCode');
    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.launchJudge' }));

    await waitFor(() => expect(compareApi.startJudge).toHaveBeenCalledWith('run-opencode-judge', {
      agent: 'OpenCode',
      tier: 'default',
      connection_id: null,
    }));
  });

  it('keeps objective and human metrics but disables AI judging and prompt improvement for mixed prompts', async () => {
    vi.clearAllMocks();
    compareApi.get.mockResolvedValue({
      run_id: 'compare-free',
      prompt_compatibility: 'different',
      improvement_availability: 'different_prompts',
      latest_judge_run: null,
      evaluations: [
        { discussion_id: 'disc-codex', manual_score: 4, manual_updated_at: null, ai: null },
        { discussion_id: 'disc-claude', manual_score: null, manual_updated_at: null, ai: null },
      ],
    });
    render(
      <BatchComparePanel
        runId="compare-free"
        label="Cross-run"
        discussions={[
          discussion('disc-codex', 'Codex', 'default', 'Codex body'),
          discussion('disc-claude', 'ClaudeCode', 'default', 'Claude body'),
        ]}
        loading={false}
        error={null}
        availableAgents={['Codex']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key, ...args) => `${key}${args.length ? ` ${args.join(' ')}` : ''}`}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.details' }));
    expect(await screen.findByText('disc.compare.judgeDisabledDifferentPrompts')).toBeInTheDocument();
    expect(screen.getByText('disc.compare.improveDisabledDifferentPrompts')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'disc.compare.launchJudge' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'disc.compare.improvePrompt' })).toBeDisabled();
    expect(screen.getAllByText('disc.compare.rankDuration')).toHaveLength(3);
    expect(screen.getAllByText('disc.compare.rankTokens')).toHaveLength(3);

    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.setHumanScore 5 Codex' }));
    await waitFor(() => expect(compareApi.updateManual).toHaveBeenCalledWith('compare-free', 'disc-codex', 5));
    expect(compareApi.startJudge).not.toHaveBeenCalled();
    expect(compareApi.startImprovement).not.toHaveBeenCalled();
  });

  it('never guesses a missing historical model from a tier configuration that changed after the run', () => {
    const noHistory = discussion('disc-no-history', 'Codex', 'reasoning', '');
    noHistory.messages = [];
    render(
      <BatchComparePanel
        runId="run-config-drifted"
        label="Config drifted after the run"
        discussions={[noHistory]}
        loading={false}
        error={null}
        availableAgents={['Codex']}
        modelTiers={{
          ...zeroModelTiers,
          codex: { economy: null, default: null, reasoning: 'codex-reasoning-v9-today' },
        }}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    // The run never recorded what actually answered; today's config must not
    // stand in for that missing history.
    expect(screen.queryByText('codex-reasoning-v9-today')).not.toBeInTheDocument();
    expect(screen.getByText('disc.defaultAgentModel')).toBeInTheDocument();
  });

  it('keeps each homonymous connection its own recorded model instead of blending them', () => {
    const fromA = discussion('disc-gateway-a', 'Custom', 'default', 'Answer from A');
    fromA.messages[0].model = 'model-from-a';
    (fromA as Discussion & { message_targets: Record<string, MessageTarget[]> }).message_targets = {
      [`m-${fromA.id}`]: [{
        kind: 'discussion_agent', agent_type: 'Custom', connection_id: 'conn-gateway-a', tier: 'default',
      }],
    };
    const fromB = discussion('disc-gateway-b', 'Custom', 'default', 'Answer from B');
    fromB.messages[0].model = 'model-from-b';
    (fromB as Discussion & { message_targets: Record<string, MessageTarget[]> }).message_targets = {
      [`m-${fromB.id}`]: [{
        kind: 'discussion_agent', agent_type: 'Custom', connection_id: 'conn-gateway-b', tier: 'default',
      }],
    };

    render(
      <BatchComparePanel
        runId="run-homonyms"
        label="Same display name, different connections"
        discussions={[fromA, fromB]}
        loading={false}
        error={null}
        availableAgents={['Custom']}
        externalConnections={[
          customConnection({ id: 'conn-gateway-a', default_model: 'a-default' }),
          customConnection({ id: 'conn-gateway-b', default_model: 'b-default' }),
        ]}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.getAllByText('Gateway')).toHaveLength(2);
    const models = Array.from(document.querySelectorAll('.disc-compare-model')).map(node => node.textContent);
    expect(models).toEqual(['model-from-a', 'model-from-b']);
  });

  it('prefers the response-attested model over an older recorded one', () => {
    const withHistory = discussion('disc-attested', 'ClaudeCode', 'default', 'Final answer');
    withHistory.messages = [
      {
        id: 'm-older', role: 'Agent', channel: 'main', content: 'Older answer',
        agent_type: 'ClaudeCode', timestamp: '2026-09-01T09:00:00Z', tokens_used: 5, duration_ms: 50,
        model: 'older-recorded-model',
      },
      withHistory.messages[0],
    ] as Discussion['messages'];
    withHistory.messages[1].model = 'final-attested-model';

    render(
      <BatchComparePanel
        runId="run-attested"
        label="Attested model wins"
        discussions={[withHistory]}
        loading={false}
        error={null}
        availableAgents={['ClaudeCode']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.getByText('final-attested-model')).toBeInTheDocument();
    expect(screen.queryByText('older-recorded-model')).not.toBeInTheDocument();
    expect(screen.queryByText('disc.compare.modelRecorded')).not.toBeInTheDocument();
    expect(screen.queryByText('disc.compare.modelUnknown')).not.toBeInTheDocument();
  });

  it('falls back to an older recorded model when the final answer model is blank, never the live discussion override', () => {
    const missingFinalModel = discussion('disc-missing-final-model', 'ClaudeCode', 'default', 'Final answer, legacy row');
    missingFinalModel.model = 'live-discussion-override';
    missingFinalModel.messages = [
      {
        id: 'm-older', role: 'Agent', channel: 'main', content: 'Older answer',
        agent_type: 'ClaudeCode', timestamp: '2026-09-01T09:00:00Z', tokens_used: 5, duration_ms: 50,
        model: 'older-recorded-model',
      },
      missingFinalModel.messages[0],
    ] as Discussion['messages'];
    missingFinalModel.messages[1].model = '   ';

    render(
      <BatchComparePanel
        runId="run-missing-final-model"
        label="Config drift must not leak"
        discussions={[missingFinalModel]}
        loading={false}
        error={null}
        availableAgents={['ClaudeCode']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.getByText('older-recorded-model')).toBeInTheDocument();
    expect(screen.queryByText('live-discussion-override')).not.toBeInTheDocument();
    expect(screen.getByText('disc.compare.modelRecorded')).toBeInTheDocument();
  });

  it('never treats a model recorded after the compared answer as evidence of what produced it', () => {
    const laterRecordAfterAnswer = discussion('disc-later-record', 'ClaudeCode', 'default', 'Final answer, blank model');
    laterRecordAfterAnswer.messages[0].model = '   ';
    laterRecordAfterAnswer.messages = [
      ...laterRecordAfterAnswer.messages,
      {
        id: 'm-system-after', role: 'System', channel: 'main', content: 'Retrying on a different backend',
        agent_type: 'ClaudeCode', timestamp: '2026-09-01T09:05:00Z', model: 'attempted-after-model',
      },
    ] as Discussion['messages'];

    render(
      <BatchComparePanel
        runId="run-later-record"
        label="A later record must not count as prior knowledge"
        discussions={[laterRecordAfterAnswer]}
        loading={false}
        error={null}
        availableAgents={['ClaudeCode']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.queryByText('attempted-after-model')).not.toBeInTheDocument();
    expect(screen.getByText('disc.defaultAgentModel')).toBeInTheDocument();
    expect(screen.getByText('disc.compare.modelUnknown')).toBeInTheDocument();
  });

  it('surfaces a System-recorded model when there is no answer at all to protect from it', () => {
    const systemOnly = discussion('disc-system-only', 'ClaudeCode', 'default', 'unused');
    systemOnly.messages = [{
      id: 'm-system-only', role: 'System', channel: 'main', content: 'Attempted before failing',
      agent_type: 'ClaudeCode', timestamp: '2026-09-01T09:00:00Z', model: 'attempted-model-no-answer',
    }] as Discussion['messages'];

    render(
      <BatchComparePanel
        runId="run-system-only"
        label="No answer, only a System record"
        discussions={[systemOnly]}
        loading={false}
        error={null}
        availableAgents={['ClaudeCode']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.getByText('attempted-model-no-answer')).toBeInTheDocument();
    expect(screen.getByText('disc.compare.modelRecorded')).toBeInTheDocument();
  });

  it('never labels the live discussion override as the model behind a run with no response at all', () => {
    const noResponse = discussion('disc-no-response-override', 'ClaudeCode', 'default', 'unused');
    noResponse.model = 'live-discussion-override';
    noResponse.messages = [];

    render(
      <BatchComparePanel
        runId="run-no-response-override"
        label="No response at all"
        discussions={[noResponse]}
        loading={false}
        error={null}
        availableAgents={['ClaudeCode']}
        runningIds={new Set()}
        onRefresh={vi.fn()}
        onOpenDiscussion={vi.fn()}
        onClose={vi.fn()}
        t={(key) => key}
      />,
    );

    expect(screen.queryByText('live-discussion-override')).not.toBeInTheDocument();
    expect(screen.getByText('disc.defaultAgentModel')).toBeInTheDocument();
    expect(screen.getByText('disc.compare.modelUnknown')).toBeInTheDocument();
  });
});

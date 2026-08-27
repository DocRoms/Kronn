import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { BatchComparePanel } from '../BatchComparePanel';
import type { Discussion } from '../../types/generated';

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
    }));
    expect(onOpenDiscussion).toHaveBeenCalledWith('disc-improvement');
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
});

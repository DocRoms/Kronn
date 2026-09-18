import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { RunOutcome, RunOutcomeDiscussion } from '../../types/generated';

const api = vi.hoisted(() => ({ outcome: vi.fn(), discussionOutcome: vi.fn() }));

vi.mock('../../lib/api', () => ({ runsApi: api }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) => args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));
vi.mock('../MessageBubble', () => ({
  MarkdownContent: ({ content }: { content: string }) => <div data-testid="markdown">{content}</div>,
}));

import { RunOutcomePanel } from '../RunOutcomePanel';

function discussion(overrides: Partial<RunOutcomeDiscussion> = {}): RunOutcomeDiscussion {
  return {
    id: 'disc-triage', title: 'EW-7791', agent: 'ClaudeCode', agent_status: 'answered',
    answer_excerpt: '**🟡 Décision requise** · confiance moyenne', answer_truncated: true,
    answered_at: '2026-09-18T15:46:34Z', diagnostic: null, updated_at: '2026-09-18T15:46:34Z',
    ...overrides,
  };
}

function outcome(discussions: RunOutcomeDiscussion[], count = discussions.length): RunOutcome {
  return { discussion_count: count, discussions };
}

beforeEach(() => {
  api.outcome.mockReset();
  api.discussionOutcome.mockReset();
});
afterEach(() => vi.useRealTimers());

describe('RunOutcomePanel', () => {
  it("tells what the run produced: the discussion, its agent's state and verdict", async () => {
    api.outcome.mockResolvedValue(outcome([discussion()], 3));
    const onOpen = vi.fn();
    render(<RunOutcomePanel runId="run-wf" runActive={false} onOpenDiscussion={onOpen} />);

    expect(await screen.findByText('EW-7791')).toBeInTheDocument();
    expect(api.outcome).toHaveBeenCalledWith('run-wf');
    expect(screen.getByTestId('run-outcome-excerpt')).toHaveTextContent('Décision requise');
    expect(screen.getByText('disc.action.outcome.answered')).toBeInTheDocument();
    expect(screen.getByText('disc.action.outcome.more:2')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /disc\.action\.openDiscussion/ }));
    expect(onOpen).toHaveBeenCalledWith('disc-triage');
  });

  it('keeps following while the agent works, and stops once it has answered', async () => {
    vi.useFakeTimers();
    api.outcome
      .mockResolvedValueOnce(outcome([discussion({ agent_status: 'working', answer_excerpt: null })]))
      .mockResolvedValue(outcome([discussion()]));
    render(<RunOutcomePanel runId="run-wf" runActive={false} onOpenDiscussion={vi.fn()} />);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(screen.getByText('disc.action.outcome.working')).toBeInTheDocument();

    await act(async () => { await vi.advanceTimersByTimeAsync(4_000); });
    expect(screen.getByText('disc.action.outcome.answered')).toBeInTheDocument();
    const calls = api.outcome.mock.calls.length;

    await act(async () => { await vi.advanceTimersByTimeAsync(12_000); });
    expect(api.outcome.mock.calls.length).toBe(calls);
  });

  it('shows why an agent failed', async () => {
    api.discussionOutcome.mockResolvedValue(outcome([
      discussion({ agent_status: 'failed', answer_excerpt: null, diagnostic: 'provider refused' }),
    ]));
    render(<RunOutcomePanel discussionId="disc-triage" runActive={false} onOpenDiscussion={vi.fn()} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('provider refused');
    expect(api.discussionOutcome).toHaveBeenCalledWith('disc-triage');
  });

  it('still leads to the result discussion when the summary cannot be read', async () => {
    api.discussionOutcome.mockRejectedValue(new Error('offline'));
    const onOpen = vi.fn();
    render(<RunOutcomePanel discussionId="disc-9" runActive={false} onOpenDiscussion={onOpen} />);
    fireEvent.click(await screen.findByRole('button', { name: /disc\.action\.openDiscussion/ }));
    expect(onOpen).toHaveBeenCalledWith('disc-9');
  });

  it('says nothing for a run that produced no discussion', async () => {
    api.outcome.mockResolvedValue(outcome([]));
    const { container } = render(<RunOutcomePanel runId="run-qe" runActive={false} onOpenDiscussion={vi.fn()} />);
    await waitFor(() => expect(api.outcome).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });
});

/**
 * KT-1490 — a Quick Exec an agent saved inactive is activated where the human
 * already is: the room card, a Page / discussion action card, and the notice a
 * workflow step shows. Only a human activates, naming the line they read.
 */
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { DiscussionAction, QuickExec, WorkflowReadiness } from '../../types/generated';

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  pendingApprovals: vi.fn(),
  activate: vi.fn(),
  refuseActivation: vi.fn(),
  readiness: vi.fn(),
  actionGet: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  quickExecs: {
    list: mocks.list,
    pendingApprovals: mocks.pendingApprovals,
    activate: mocks.activate,
    refuseActivation: mocks.refuseActivation,
  },
  workflows: { get: vi.fn(), readiness: mocks.readiness },
  discussionActions: { get: mocks.actionGet, cancel: vi.fn(), launch: vi.fn(), list: vi.fn() },
  agents: { detect: vi.fn() },
  runsApi: { outcome: vi.fn(), discussionOutcome: vi.fn() },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));
vi.mock('../RunStatusCard', () => ({ RunStatusCard: () => null }));
vi.mock('../AgentSwitchPicker', () => ({ AgentSwitchPicker: () => null }));
vi.mock('../ModelCatalogPicker', () => ({ ModelCatalogPicker: () => null }));

import {
  PendingActivationNotice,
  QuickExecActivationCard,
} from '../QuickExecActivation';
import { parseActivationCard } from '../../lib/activationCard';
import { DiscussionActionCard } from '../DiscussionActionCard';

const purge: QuickExec = {
  id: 'qe-1',
  name: 'Purge CDN',
  icon: '⌘',
  description: '',
  project_id: null,
  command: 'fastly',
  args: ['purge', '--key', '{{surrogate_key}}'],
  timeout_secs: 60,
  output_format: 'text',
  variables: [],
  pinned: false,
  unmodelled_args_approved: undefined,
  agent_written: true,
  created_at: '2026-10-28T10:00:00Z',
  updated_at: '2026-10-28T10:00:00Z',
} as QuickExec;

const waiting = { id: 'qe-1', program: 'fastly', message: 'en attente', refused: false };

beforeEach(() => {
  Object.values(mocks).forEach(mock => mock.mockReset());
  mocks.list.mockResolvedValue([purge]);
  mocks.pendingApprovals.mockResolvedValue([waiting]);
  mocks.activate.mockResolvedValue({ ...purge, unmodelled_args_approved: true });
  mocks.refuseActivation.mockResolvedValue(true);
  mocks.readiness.mockResolvedValue({ blockers: [] } as unknown as WorkflowReadiness);
});

describe('the room activation card', () => {
  const card = { quick_exec_id: 'qe-1', name: 'Purge CDN', program: 'fastly' };

  it('reads the card a System message carries, and nothing else', () => {
    const content = `[kronn-activate: ${JSON.stringify(card)}]`;
    expect(parseActivationCard(content)).toEqual(card);
    expect(parseActivationCard('[kronn-planning: {}]')).toBeNull();
    expect(parseActivationCard('[kronn-activate: not json]')).toBeNull();
    expect(parseActivationCard('[kronn-activate: {"name":"x"}]')).toBeNull();
  });

  it('offers Activer / Refuser and activates the line the human read', async () => {
    render(<QuickExecActivationCard card={card} />);
    expect(await screen.findByText('qeActivation.title:Purge CDN/fastly')).toBeDefined();
    expect(screen.getByText('fastly purge --key {{surrogate_key}}')).toBeDefined();
    expect(mocks.activate).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText('qeActivation.activate'));
    await waitFor(() => expect(mocks.activate).toHaveBeenCalledWith('qe-1', {
      command: 'fastly',
      args: ['purge', '--key', '{{surrogate_key}}'],
    }));
    expect(await screen.findByText('qeActivation.done:Purge CDN')).toBeDefined();
  });

  it('records the refusal server-side, naming the line read, and keeps the Quick Exec inactive', async () => {
    render(<QuickExecActivationCard card={card} />);
    fireEvent.click(await screen.findByText('qeActivation.refuse'));
    await waitFor(() => expect(mocks.refuseActivation).toHaveBeenCalledWith('qe-1', {
      command: 'fastly',
      args: ['purge', '--key', '{{surrogate_key}}'],
    }));
    expect(await screen.findByText('qeActivation.refused:Purge CDN')).toBeDefined();
    expect(mocks.activate).not.toHaveBeenCalled();
  });

  it('is still « Refusée » after a reload, because the server says so', async () => {
    mocks.pendingApprovals.mockResolvedValue([{ ...waiting, refused: true }]);
    render(<QuickExecActivationCard card={card} />);
    expect(await screen.findByText('qeActivation.refused:Purge CDN')).toBeDefined();
    expect(screen.queryByText('qeActivation.activate')).toBeNull();
    expect(mocks.refuseActivation).not.toHaveBeenCalled();
  });

  it('stays in « en attente » with the error when the server does not record the refusal', async () => {
    mocks.refuseActivation.mockRejectedValue(new Error('Only a human refuses'));
    render(<QuickExecActivationCard card={card} />);
    fireEvent.click(await screen.findByText('qeActivation.refuse'));
    expect((await screen.findByRole('alert')).textContent).toContain('Only a human refuses');
    expect(screen.queryByText('qeActivation.refused:Purge CDN')).toBeNull();
    expect(screen.getByText('qeActivation.activate')).toBeDefined();
  });

  it('shows the refusal when the line changed since it was read', async () => {
    mocks.activate.mockRejectedValue(new Error('La ligne a changé'));
    render(<QuickExecActivationCard card={card} />);
    fireEvent.click(await screen.findByText('qeActivation.activate'));
    expect((await screen.findByRole('alert')).textContent).toContain('La ligne a changé');
  });

  const rewritten = ['service', 'delete', '--service', '{{id}}'];

  it('reads the line again after a refused activation and shows the new one', async () => {
    render(<QuickExecActivationCard card={card} />);
    expect(await screen.findByText('fastly purge --key {{surrogate_key}}')).toBeDefined();
    // The agent rewrites the line between render and click; the server refuses.
    mocks.list.mockResolvedValue([{ ...purge, args: rewritten }]);
    mocks.activate.mockRejectedValue(new Error('La ligne de cette Quick Exec a changé'));
    fireEvent.click(screen.getByText('qeActivation.activate'));
    expect((await screen.findByRole('alert')).textContent).toContain('a changé');
    expect(await screen.findByText('fastly service delete --service {{id}}')).toBeDefined();
    expect(screen.queryByText('fastly purge --key {{surrogate_key}}')).toBeNull();
    expect(mocks.activate).toHaveBeenCalledTimes(1);
    expect(mocks.activate).toHaveBeenCalledWith('qe-1', {
      command: 'fastly',
      args: ['purge', '--key', '{{surrogate_key}}'],
    });
  });

  it('reads the line again after a refused refusal and shows the new one', async () => {
    render(<QuickExecActivationCard card={card} />);
    expect(await screen.findByText('fastly purge --key {{surrogate_key}}')).toBeDefined();
    mocks.list.mockResolvedValue([{ ...purge, args: rewritten }]);
    mocks.refuseActivation.mockRejectedValue(new Error('La ligne de cette Quick Exec a changé'));
    fireEvent.click(screen.getByText('qeActivation.refuse'));
    expect((await screen.findByRole('alert')).textContent).toContain('a changé');
    expect(await screen.findByText('fastly service delete --service {{id}}')).toBeDefined();
    expect(screen.queryByText('fastly purge --key {{surrogate_key}}')).toBeNull();
    expect(mocks.refuseActivation).toHaveBeenCalledTimes(1);
  });

  it('shows each argument with its boundaries, so two different argv never read the same', async () => {
    const split = ['purge', '--key', '{{id}}'];
    const joined = ['purge --key', '{{id}}'];
    mocks.list.mockResolvedValue([{ ...purge, args: split }]);
    const first = render(<QuickExecActivationCard card={card} />);
    await screen.findByText('qeActivation.title:Purge CDN/fastly');
    const shownSplit = first.container.querySelector('.qe-activation-card__line')?.textContent;
    first.unmount();

    mocks.list.mockResolvedValue([{ ...purge, args: joined }]);
    const second = render(<QuickExecActivationCard card={card} />);
    await screen.findByText('qeActivation.title:Purge CDN/fastly');
    const shownJoined = second.container.querySelector('.qe-activation-card__line')?.textContent;

    expect(shownSplit).toBe('fastly purge --key {{id}}');
    expect(shownJoined).toBe("fastly 'purge --key' {{id}}");
    expect(shownJoined).not.toBe(shownSplit);
    // What is sent is the vector the human was shown.
    fireEvent.click(screen.getByText('qeActivation.activate'));
    await waitFor(() => expect(mocks.activate).toHaveBeenCalledWith('qe-1', {
      command: 'fastly',
      args: joined,
    }));
  });

  it('shows a Quick Exec that is already active as done, without a button', async () => {
    mocks.pendingApprovals.mockResolvedValue([]);
    render(<QuickExecActivationCard card={card} />);
    expect(await screen.findByText('qeActivation.done:Purge CDN')).toBeDefined();
    expect(screen.queryByText('qeActivation.activate')).toBeNull();
  });
});

const pendingPurge = {
  id: 'qe-1',
  name: 'Purge CDN',
  program: 'fastly',
  command: 'fastly',
  args: ['purge', '--key', '{{surrogate_key}}'],
};

describe('the pending-activation notice', () => {
  it('shows the line it will activate and sends exactly that line, never a re-read one', async () => {
    // The agent rewrites the saved line after the notice rendered.
    mocks.list.mockResolvedValue([{ ...purge, args: ['service', 'delete', '--service', '{{id}}'] }]);
    render(<PendingActivationNotice items={[pendingPurge]} />);
    // The line sits in the room card's code block, not in bare inline code.
    const line = screen.getByText('fastly purge --key {{surrogate_key}}');
    expect(line.tagName).toBe('PRE');
    expect(line.classList.contains('qe-command')).toBe(true);
    fireEvent.click(screen.getByText('qeActivation.activateNamed:Purge CDN'));
    await waitFor(() => expect(mocks.activate).toHaveBeenCalledWith('qe-1', {
      command: 'fastly',
      args: ['purge', '--key', '{{surrogate_key}}'],
    }));
    expect(mocks.list).not.toHaveBeenCalled();
  });

  it('shows each argument with its boundaries in the notice too', () => {
    render(<PendingActivationNotice items={[{ ...pendingPurge, args: ['purge --key', '{{id}}'] }]} />);
    expect(screen.getByText("fastly 'purge --key' {{id}}")).toBeDefined();
    expect(screen.queryByText('fastly purge --key {{id}}')).toBeNull();
  });

  it('is refused when the line changed, says so, and asks the caller to show the new line', async () => {
    mocks.activate.mockRejectedValue(new Error('La ligne de cette Quick Exec a changé'));
    const onActivated = vi.fn();
    const onRefresh = vi.fn();
    render(<PendingActivationNotice items={[pendingPurge]} onActivated={onActivated} onRefresh={onRefresh} />);
    fireEvent.click(screen.getByText('qeActivation.activateNamed:Purge CDN'));
    expect((await screen.findByRole('alert')).textContent).toContain('a changé');
    expect(onRefresh).toHaveBeenCalledTimes(1);
    expect(onActivated).not.toHaveBeenCalled();
  });

  it('says the step waits and activates it with one click', async () => {
    const onActivated = vi.fn();
    render(
      <PendingActivationNotice
        items={[pendingPurge]}
        onActivated={onActivated}
      />,
    );
    expect(screen.getByText('qeActivation.pending:Purge CDN')).toBeDefined();
    fireEvent.click(screen.getByText('qeActivation.activateNamed:Purge CDN'));
    await waitFor(() => expect(mocks.activate).toHaveBeenCalledWith('qe-1', {
      command: 'fastly',
      args: ['purge', '--key', '{{surrogate_key}}'],
    }));
    await waitFor(() => expect(onActivated).toHaveBeenCalled());
  });

  it('renders nothing when nothing waits', () => {
    const { container } = render(<PendingActivationNotice items={[]} />);
    expect(container.textContent).toBe('');
  });
});

describe('a Page / discussion action on an inactive Quick Exec', () => {
  const base: DiscussionAction = {
    id: 'action:msg-1:0',
    discussion_id: 'disc-1',
    source_message_id: 'msg-1',
    fence_index: 0,
    kind: 'quick_exec',
    target_id: 'qe-1',
    target_name: 'Purge CDN',
    project_id: null,
    project_name: null,
    state: 'proposed',
    values: [],
    shared_run_id: null,
    result_discussion_id: null,
    deep_link: null,
    diagnostic: null,
    launched_at: null,
    finished_at: null,
    created_at: '2026-10-28T10:00:00Z',
    updated_at: '2026-10-28T10:00:00Z',
  };

  it('shows « en attente d\'activation » with the activate button instead of failing', async () => {
    render(<DiscussionActionCard action={base} onChanged={vi.fn()} onOpenDiscussion={vi.fn()} />);
    expect(await screen.findByText('qeActivation.pending:Purge CDN')).toBeDefined();
    fireEvent.click(screen.getByText('qeActivation.activateNamed:Purge CDN'));
    await waitFor(() => expect(mocks.activate).toHaveBeenCalledWith('qe-1', expect.objectContaining({
      command: 'fastly',
    })));
    await waitFor(() => expect(mocks.pendingApprovals).toHaveBeenCalledTimes(2));
  });

  it('activates the line shown; a line changed between render and click is refused and the new line shown', async () => {
    render(<DiscussionActionCard action={base} onChanged={vi.fn()} onOpenDiscussion={vi.fn()} />);
    expect(await screen.findByText('fastly purge --key {{surrogate_key}}')).toBeDefined();
    // The agent rewrites the line; the server refuses what the human read.
    const rewritten = ['service', 'delete', '--service', '{{id}}'];
    mocks.list.mockResolvedValue([{ ...purge, args: rewritten }]);
    mocks.activate.mockRejectedValue(new Error('La ligne de cette Quick Exec a changé'));
    fireEvent.click(screen.getByText('qeActivation.activateNamed:Purge CDN'));
    await waitFor(() => expect(mocks.activate).toHaveBeenCalledWith('qe-1', {
      command: 'fastly',
      args: ['purge', '--key', '{{surrogate_key}}'],
    }));
    expect(mocks.activate).toHaveBeenCalledTimes(1);
    expect((await screen.findByRole('alert')).textContent).toContain('a changé');
    // The notice now asks about the new line; nothing was activated meanwhile.
    expect(await screen.findByText('fastly service delete --service {{id}}')).toBeDefined();
    expect(screen.queryByText('fastly purge --key {{surrogate_key}}')).toBeNull();
  });

  it('shows nothing once the Quick Exec is active', async () => {
    mocks.pendingApprovals.mockResolvedValue([]);
    render(<DiscussionActionCard action={base} onChanged={vi.fn()} onOpenDiscussion={vi.fn()} />);
    await waitFor(() => expect(mocks.pendingApprovals).toHaveBeenCalled());
    expect(screen.queryByTestId('qe-pending-activation')).toBeNull();
  });

  it('names the Quick Exec and the Exec step of a workflow that wait', async () => {
    mocks.readiness.mockResolvedValue({
      blockers: [
        {
          workflow_id: 'wf-1', workflow_name: 'Purge', step: 'collect', on_failure: false,
          kind: 'human_approval', phase: 'source', reason: 'unmodelled_program', message: '',
          action: '', human_only: true,
          quick_exec: { id: 'qe-1', name: 'Purge CDN', source_alias: 'q', program: 'fastly' },
        },
        {
          workflow_id: 'wf-1', workflow_name: 'Purge', step: 'purge-line', on_failure: false,
          kind: 'human_approval', phase: 'main', reason: 'unmodelled_program', message: '',
          action: '', human_only: true,
        },
      ],
    } as unknown as WorkflowReadiness);
    render(
      <DiscussionActionCard
        action={{ ...base, kind: 'workflow', target_id: 'wf-1', target_name: 'Purge' }}
        onChanged={vi.fn()}
        onOpenDiscussion={vi.fn()}
      />,
    );
    expect(await screen.findByText('qeActivation.pending:Purge CDN')).toBeDefined();
    expect(screen.getByText('fastly purge --key {{surrogate_key}}')).toBeDefined();
    expect(screen.getByText('qeActivation.stepPending:purge-line')).toBeDefined();
    expect(screen.getByText('qeActivation.openWorkflow').getAttribute('href')).toContain('wf-1');
  });
});

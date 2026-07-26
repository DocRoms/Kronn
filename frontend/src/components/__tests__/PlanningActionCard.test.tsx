import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

const mocks = vi.hoisted(() => ({
  proposal: vi.fn(),
  decideProposalItem: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  planning: mocks,
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));

import { PlanningActionCard } from '../PlanningActionCard';
import { parsePlanningProposal } from '../../lib/planningProposal';

describe('PlanningActionCard', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('rejects malformed action payloads before they become buttons', () => {
    expect(parsePlanningProposal({ action: 'create' })).toBeNull();
    expect(parsePlanningProposal({ action: 'delete', task_id: 'KT-1' })).toBeNull();
    expect(parsePlanningProposal({ action: 'status', task_id: 'KT-1', status: 'deleted' })).toBeNull();
    expect(parsePlanningProposal({ action: 'create', title: 'Task', priority: 'urgent' })).toBeNull();
    expect(parsePlanningProposal({ action: 'complete', task_id: 'KT-1' })).toEqual({
      action: 'complete',
      task_id: 'KT-1',
    });
  });

  it('keeps a durable create proposal behind an idempotent human decision', async () => {
    mocks.proposal.mockResolvedValue({
      id: 'proposal:message-1:0',
      discussion_id: 'disc-1',
      source_message_id: 'message-1',
      fence_index: 0,
      aggregate_state: 'pending',
      items: [{
        id: 'proposal:message-1:0:item:0',
        item_index: 0,
        action: 'create',
        payload: { title: 'Upgrade PHP', priority: 'high', is_primary: true },
        state: 'pending',
      }],
      created_at: '2026-07-26T00:00:00Z',
      updated_at: '2026-07-26T00:00:00Z',
    });
    mocks.decideProposalItem.mockResolvedValue({
      item: {
        id: 'proposal:message-1:0:item:0',
        item_index: 0,
        action: 'create',
        payload: { title: 'Upgrade PHP', priority: 'high', is_primary: true },
        state: 'accepted',
        result_task_id: 'task-new',
      },
      aggregate_state: 'applied',
    });
    render(
      <PlanningActionCard
        discussionId="disc-1"
        sourceMessageId="message-1"
        fenceIndex={0}
        proposal={{
          action: 'create',
          title: 'Upgrade PHP',
          priority: 'high',
          is_primary: true,
        }}
      />,
    );

    expect(mocks.decideProposalItem).not.toHaveBeenCalled();
    fireEvent.click(await screen.findByText('planning.acceptProposal'));
    await waitFor(() => expect(mocks.decideProposalItem).toHaveBeenCalledWith(
      'proposal:message-1:0',
      'proposal:message-1:0:item:0',
      expect.objectContaining({
        decision: 'accept',
        idempotency_key: expect.any(String),
      }),
    ));
    expect(await screen.findByText('planning.proposalItem.accepted')).toBeInTheDocument();
  });

  it('previews every durable item title before applying a multi-task proposal', async () => {
    mocks.proposal.mockResolvedValue({
      id: 'proposal:message-1:0',
      discussion_id: 'disc-1',
      source_message_id: 'message-1',
      fence_index: 0,
      aggregate_state: 'pending',
      items: ['Serve stable', 'Persist agent jobs', 'Make edit and resend atomic']
        .map((title, item_index) => ({
          id: `proposal:message-1:0:item:${item_index}`,
          item_index,
          action: 'create',
          payload: { title },
          state: 'pending',
        })),
      created_at: '2026-07-26T00:00:00Z',
      updated_at: '2026-07-26T00:00:00Z',
    });
    render(
      <PlanningActionCard
        discussionId="disc-1"
        sourceMessageId="message-1"
        fenceIndex={0}
        proposal={{
          action: 'create_many',
          tasks: [
            { title: 'Serve stable' },
            { title: 'Persist agent jobs' },
            { title: 'Make edit and resend atomic' },
          ],
        }}
      />,
    );

    expect(screen.getByText('planning.proposalCreateMany:3')).toBeInTheDocument();
    expect(await screen.findByText('Serve stable')).toBeInTheDocument();
    expect(screen.getByText('Persist agent jobs')).toBeInTheDocument();
    expect(screen.getByText('Make edit and resend atomic')).toBeInTheDocument();
    expect(mocks.decideProposalItem).not.toHaveBeenCalled();
  });

  it('reuses the idempotency key when a failed decision is retried', async () => {
    mocks.proposal.mockResolvedValue({
      id: 'proposal:message-1:0',
      discussion_id: 'disc-1',
      source_message_id: 'message-1',
      fence_index: 0,
      aggregate_state: 'pending',
      items: [{
        id: 'proposal:message-1:0:item:0',
        item_index: 0,
        action: 'complete',
        payload: { task_id: 'task-1' },
        state: 'pending',
      }],
      created_at: '2026-07-26T00:00:00Z',
      updated_at: '2026-07-26T00:00:00Z',
    });
    mocks.decideProposalItem
      .mockRejectedValueOnce(new Error('offline'))
      .mockResolvedValueOnce({
        item: {
          id: 'proposal:message-1:0:item:0',
          item_index: 0,
          action: 'complete',
          payload: { task_id: 'task-1' },
          state: 'accepted',
        },
        aggregate_state: 'applied',
      });

    render(
      <PlanningActionCard
        discussionId="disc-1"
        sourceMessageId="message-1"
        fenceIndex={0}
        proposal={{ action: 'complete', task_id: 'task-1' }}
      />,
    );

    fireEvent.click(await screen.findByText('planning.acceptProposal'));
    const retry = await screen.findByText('planning.retry');
    const firstKey = mocks.decideProposalItem.mock.calls[0][2].idempotency_key;
    fireEvent.click(retry);
    await waitFor(() => expect(mocks.decideProposalItem).toHaveBeenCalledTimes(2));
    expect(mocks.decideProposalItem.mock.calls[1][2].idempotency_key).toBe(firstKey);
  });

  it('refreshes when the same proposal is decided from another UI surface', async () => {
    const pending = {
      id: 'proposal:message-1:0',
      discussion_id: 'disc-1',
      source_message_id: 'message-1',
      fence_index: 0,
      aggregate_state: 'pending',
      items: [{
        id: 'proposal:message-1:0:0',
        item_index: 0,
        action: 'create',
        payload: { title: 'Cross-surface task' },
        state: 'pending',
      }],
      created_at: '2026-07-26T00:00:00Z',
      updated_at: '2026-07-26T00:00:00Z',
    };
    mocks.proposal
      .mockResolvedValueOnce(pending)
      .mockResolvedValueOnce({
        ...pending,
        aggregate_state: 'applied',
        items: [{ ...pending.items[0], state: 'accepted' }],
      });

    render(
      <PlanningActionCard
        discussionId="disc-1"
        sourceMessageId="message-1"
        fenceIndex={0}
        proposal={{ action: 'create', title: 'Cross-surface task' }}
      />,
    );
    expect(await screen.findByText('planning.proposalItem.pending')).toBeInTheDocument();

    window.dispatchEvent(new CustomEvent('kronn:plan-proposals-changed', {
      detail: { discussionId: 'disc-1' },
    }));

    expect(await screen.findByText('planning.proposalItem.accepted')).toBeInTheDocument();
    expect(mocks.proposal).toHaveBeenCalledTimes(2);
  });
});

import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';

const mocks = vi.hoisted(() => ({
  create: vi.fn(),
  linkDiscussion: vi.fn(),
  update: vi.fn(),
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

  it('keeps an ambiguous create proposal behind a human click', async () => {
    mocks.create.mockResolvedValue({ id: 'task-new' });
    mocks.linkDiscussion.mockResolvedValue({});
    render(
      <PlanningActionCard
        discussionId="disc-1"
        proposal={{
          action: 'create',
          title: 'Upgrade PHP',
          priority: 'high',
          is_primary: true,
        }}
      />,
    );

    expect(mocks.create).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText('planning.applyProposal'));
    await waitFor(() => expect(mocks.create).toHaveBeenCalledWith({
      title: 'Upgrade PHP',
      description: '',
      priority: 'high',
      status: 'todo',
    }));
    expect(mocks.linkDiscussion).toHaveBeenCalledWith('task-new', {
      discussion_id: 'disc-1',
      placement: 'active',
      is_primary: true,
    });
    expect(await screen.findByText('planning.applied')).toBeInTheDocument();
  });
});

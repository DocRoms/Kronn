import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

const mocks = vi.hoisted(() => ({
  discussionPlan: vi.fn(),
  create: vi.fn(),
  linkDiscussion: vi.fn(),
  update: vi.fn(),
  updateDod: vi.fn(),
  get: vi.fn(),
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

import { DiscussionPlanPanel } from '../DiscussionPlanPanel';
import type {
  DiscussionPlan,
  PlanningTaskDetail,
  PlanningTaskSummary,
} from '../../types/generated';

function task(overrides: Partial<PlanningTaskSummary> = {}): PlanningTaskSummary {
  return {
    id: 'task-1',
    reference: 'KT-1',
    parent_id: null,
    parent_reference: null,
    parent_title: null,
    title: 'Build the panel',
    status: 'todo',
    priority: 'normal',
    rank: 1024,
    completed_subtasks: 0,
    total_subtasks: 0,
    project_ids: [],
    discussion_ids: ['disc-1'],
    tags: [],
    blocker_count: 0,
    created_at: '2026-07-25T00:00:00Z',
    updated_at: '2026-07-25T00:00:00Z',
    ...overrides,
  };
}

function plan(overrides: Partial<DiscussionPlan> = {}): DiscussionPlan {
  return {
    discussion_id: 'disc-1',
    primary_objective: task(),
    active: [{ placement: 'active', is_primary: true, position: 0, task: task() }],
    later: [],
    completed_active: 0,
    total_active: 1,
    ...overrides,
  };
}

function detail(overrides: Partial<PlanningTaskDetail> = {}): PlanningTaskDetail {
  return {
    ...task(),
    subtasks: [],
    description: '',
    blocked_reason: null,
    definition_of_done: [],
    links: [],
    blockers: [],
    blocking: [],
    events: [],
    ...overrides,
  };
}

describe('DiscussionPlanPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.discussionPlan.mockResolvedValue(plan());
    mocks.update.mockResolvedValue({});
    mocks.updateDod.mockResolvedValue({});
    mocks.get.mockResolvedValue(detail());
  });

  it('renders the primary objective and compact active timeline', async () => {
    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );
    expect(await screen.findAllByText('Build the panel')).toHaveLength(2);
    expect(screen.getAllByText('KT-1')).toHaveLength(2);
    expect(screen.getByText('planning.progress:0/1')).toBeInTheDocument();
    expect(screen.getAllByTitle('planning.copyTaskId:KT-1')).toHaveLength(2);
  });

  it('refreshes an open plan in the background', async () => {
    vi.useFakeTimers();
    try {
      render(
        <DiscussionPlanPanel
          discussionId="disc-1"
          onClose={vi.fn()}
          toast={vi.fn()}
        />,
      );
      await act(async () => {
        await Promise.resolve();
      });
      expect(mocks.discussionPlan).toHaveBeenCalledTimes(1);

      await act(async () => {
        vi.advanceTimersByTime(5_000);
        await Promise.resolve();
      });
      expect(mocks.discussionPlan).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('moves a task between active and later without recreating it', async () => {
    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );
    fireEvent.click(await screen.findByTitle('planning.moveLater'));
    await waitFor(() => expect(mocks.linkDiscussion).toHaveBeenCalledWith('task-1', {
      discussion_id: 'disc-1',
      placement: 'later',
      is_primary: true,
    }));
  });

  it('quick-creates a todo and links it as the first primary objective', async () => {
    const empty = plan({
      primary_objective: null,
      active: [],
      later: [],
      total_active: 0,
    });
    const created = task({ id: 'task-new', reference: 'KT-2', title: 'New task' });
    mocks.discussionPlan.mockResolvedValue(empty);
    mocks.create.mockResolvedValue(created);
    mocks.linkDiscussion.mockResolvedValue(plan({
      primary_objective: created,
      active: [{ placement: 'active', is_primary: true, position: 0, task: created }],
    }));

    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );
    const input = await screen.findByPlaceholderText('planning.quickCreate');
    fireEvent.change(input, { target: { value: 'New task' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(mocks.create).toHaveBeenCalledWith({
      title: 'New task',
      status: 'todo',
    }));
    expect(mocks.linkDiscussion).toHaveBeenCalledWith('task-new', {
      discussion_id: 'disc-1',
      placement: 'active',
      is_primary: true,
    });
  });

  it('checks a task by changing its global status to done', async () => {
    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );
    const check = await screen.findByLabelText('planning.markDone');
    fireEvent.click(check);
    await waitFor(() => expect(mocks.update).toHaveBeenCalledWith('task-1', {
      status: 'done',
    }));
    expect(mocks.discussionPlan).toHaveBeenCalledTimes(2);
  });

  it('checks a Definition of Done item and shows its updated progress', async () => {
    const initial = detail({
      definition_of_done: [{
        id: 'dod-1',
        sentence: 'Ship the focused test',
        completed: false,
        position: 0,
      }],
    });
    const updated = detail({
      definition_of_done: [{
        id: 'dod-2',
        sentence: 'Ship the focused test',
        completed: true,
        position: 0,
      }],
    });
    mocks.get.mockResolvedValueOnce(initial).mockResolvedValue(updated);
    mocks.updateDod.mockResolvedValue(updated);
    const { container } = render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );
    await screen.findByText('planning.progress:0/1');
    fireEvent.click(container.querySelector('.plan-task-main') as HTMLElement);
    const dod = await screen.findByLabelText('planning.checkDod:Ship the focused test');
    fireEvent.click(dod);
    await waitFor(() => expect(mocks.updateDod).toHaveBeenCalledWith(
      'task-1',
      'dod-1',
      { completed: true },
    ));
    expect(await screen.findByText(/planning\.definitionOfDone · 1\/1/)).toBeInTheDocument();
  });
});

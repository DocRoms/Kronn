import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';

const mocks = vi.hoisted(() => ({
  discussionPlan: vi.fn(),
  create: vi.fn(),
  linkDiscussion: vi.fn(),
  update: vi.fn(),
  updateDod: vi.fn(),
  get: vi.fn(),
  proposals: vi.fn(),
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

import { DiscussionPlanPanel } from '../DiscussionPlanPanel';
import type {
  DiscussionPlan,
  PlanningDependencySummary,
  PlanningDiscussionRelation,
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

function relation(
  taskOverrides: Partial<PlanningTaskSummary> = {},
  overrides: Partial<PlanningDiscussionRelation> = {},
): PlanningDiscussionRelation {
  return {
    placement: 'active',
    is_primary: false,
    position: 0,
    task: task(taskOverrides),
    active_blockers: [],
    actionable: taskOverrides.status === undefined || taskOverrides.status === 'todo',
    ...overrides,
  };
}

function plan(overrides: Partial<DiscussionPlan> = {}): DiscussionPlan {
  return {
    discussion_id: 'disc-1',
    primary_objective: task(),
    active: [
      relation({}, { is_primary: true }),
    ],
    later: [],
    completed_active: 0,
    total_active: 1,
    stats: { ready: 1, blocked: 0, in_progress: 0, ideas: 0, done: 0, later: 0 },
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
    mocks.proposals.mockResolvedValue({
      proposals: [],
      pending_proposal_count: 0,
      pending_item_count: 0,
    });
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
      active: [
        relation(created, { is_primary: true }),
      ],
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
    fireEvent.click(container.querySelector('.plan-task-open') as HTMLElement);
    const dod = await screen.findByLabelText('planning.checkDod:Ship the focused test');
    fireEvent.click(dod);
    await waitFor(() => expect(mocks.updateDod).toHaveBeenCalledWith(
      'task-1',
      'dod-1',
      { completed: true },
    ));
    expect(await screen.findByText(/planning\.definitionOfDone · 1\/1/)).toBeInTheDocument();
  });

  it('shows at most three current and five actionable tasks in plan order', async () => {
    const blockedCurrent = relation({
      id: 'current-blocked',
      reference: 'KT-9',
      title: 'Current but blocked',
      status: 'in_progress',
      blocker_count: 1,
    }, {
      position: 0,
      active_blockers: [{
        id: 'blocker-current',
        reference: 'KT-8',
        title: 'Current blocker',
        status: 'todo',
        project_ids: [],
        discussion_ids: [],
      }],
      actionable: false,
    });
    const current = Array.from({ length: 4 }, (_, index) => relation({
      id: `current-${index}`,
      reference: `KT-${index + 10}`,
      title: `Current ${index + 1}`,
      status: 'in_progress',
    }, {
      position: index,
      actionable: false,
    }));
    const ready = Array.from({ length: 7 }, (_, index) => relation({
      id: `ready-${index}`,
      reference: `KT-${index + 20}`,
      title: `Ready ${index + 1}`,
    }, {
      position: index + current.length,
      actionable: true,
    }));
    mocks.discussionPlan.mockResolvedValue(plan({
      primary_objective: null,
      active: [blockedCurrent, ...current, ...ready],
      total_active: 12,
      stats: {
        ready: 7,
        blocked: 3,
        in_progress: 4,
        ideas: 1,
        done: 3,
        later: 6,
      },
    }));

    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );

    expect(await screen.findByText('Current 1')).toBeInTheDocument();
    expect(screen.getByText('Current 3')).toBeInTheDocument();
    expect(screen.queryByText('Current but blocked')).not.toBeInTheDocument();
    expect(screen.queryByText('Current 4')).not.toBeInTheDocument();
    expect(screen.getByText('planning.moreCurrent:1')).toBeInTheDocument();
    expect(screen.getByText('Ready 1')).toBeInTheDocument();
    expect(screen.getByText('Ready 5')).toBeInTheDocument();
    expect(screen.queryByText('Ready 6')).not.toBeInTheDocument();
    expect(screen.getByText('planning.moreReady:2')).toBeInTheDocument();
    expect(screen.getByText('planning.focusReady:7')).toBeInTheDocument();
    expect(screen.getByText('planning.focusBlocked:3')).toBeInTheDocument();
    expect(screen.getByText('planning.focusDone:3')).toBeInTheDocument();
    expect(screen.getByText('planning.focusIdeas:1')).toBeInTheDocument();
    expect(screen.getByText('planning.focusLater:6')).toBeInTheDocument();
  });

  it('promotes a task into Up next when its final dependency is completed', async () => {
    const blocker: PlanningDependencySummary = {
      id: 'blocker-1',
      reference: 'KT-90',
      title: 'External blocker',
      status: 'todo',
      project_ids: [],
      discussion_ids: ['disc-other'],
    };
    const waiting = relation({
      id: 'waiting',
      reference: 'KT-91',
      title: 'Waiting task',
      blocker_count: 1,
    }, {
      active_blockers: [blocker],
      actionable: false,
    });
    const unblocked = relation({
      id: 'waiting',
      reference: 'KT-91',
      title: 'Waiting task',
      blocker_count: 0,
    }, {
      actionable: true,
    });
    mocks.discussionPlan
      .mockResolvedValueOnce(plan({
        primary_objective: null,
        active: [waiting],
        stats: { ready: 0, blocked: 1, in_progress: 0, ideas: 0, done: 0, later: 0 },
      }))
      .mockResolvedValue(plan({
        primary_objective: null,
        active: [unblocked],
        stats: { ready: 1, blocked: 0, in_progress: 0, ideas: 0, done: 0, later: 0 },
      }));

    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );

    expect(await screen.findByText('planning.noActionable')).toBeInTheDocument();
    expect(screen.queryByText('Waiting task')).not.toBeInTheDocument();
    window.dispatchEvent(new CustomEvent('kronn:plan-proposals-changed', {
      detail: { discussionId: 'disc-1' },
    }));
    expect(await screen.findByText('Waiting task')).toBeInTheDocument();
    expect(screen.queryByText('planning.noActionable')).not.toBeInTheDocument();
  });

  it('indents only explicitly linked subtasks and keeps orphan parent context', async () => {
    const parent = relation({
      id: 'parent',
      reference: 'KT-100',
      title: 'Linked parent',
    });
    const child = relation({
      id: 'child',
      reference: 'KT-101',
      title: 'Linked child',
      parent_id: 'parent',
      parent_reference: 'KT-100',
      parent_title: 'Linked parent',
    }, {
      position: 1,
    });
    const orphan = relation({
      id: 'orphan',
      reference: 'KT-102',
      title: 'Orphan child',
      parent_id: 'outside-plan',
      parent_reference: 'KT-99',
      parent_title: 'Outside parent',
    }, {
      position: 2,
    });
    mocks.discussionPlan.mockResolvedValue(plan({
      primary_objective: null,
      active: [parent, child, orphan],
      total_active: 3,
      stats: { ready: 3, blocked: 0, in_progress: 0, ideas: 0, done: 0, later: 0 },
    }));

    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );

    const linkedChild = (await screen.findByText('Linked child')).closest('.plan-task');
    const orphanChild = screen.getByText('Orphan child').closest('.plan-task');
    expect(linkedChild).toHaveAttribute('data-depth', '1');
    expect(orphanChild).toHaveAttribute('data-depth', '0');
    expect(screen.getByText('KT-99 · Outside parent')).toBeInTheDocument();
  });

  it('loads one detail per selection and renders internal and external dependency targets', async () => {
    const internal = relation({
      id: 'task-internal',
      reference: 'KT-200',
      title: 'Internal blocker',
    }, {
      position: 1,
    });
    const selectedDetail = detail({
      blockers: [
        task({
          id: 'task-internal',
          reference: 'KT-200',
          title: 'Internal blocker',
        }),
        task({
          id: 'task-external',
          reference: 'KT-201',
          title: 'External blocker',
          project_ids: ['project-external'],
          discussion_ids: ['disc-external'],
        }),
      ],
      blocking: [
        task({
          id: 'task-blocked',
          reference: 'KT-202',
          title: 'Downstream task',
          status: 'blocked',
        }),
      ],
    });
    mocks.discussionPlan.mockResolvedValue(plan({
      active: [relation({}, { is_primary: true }), internal],
      total_active: 2,
      stats: { ready: 2, blocked: 0, in_progress: 0, ideas: 0, done: 0, later: 0 },
    }));
    mocks.get
      .mockResolvedValueOnce(selectedDetail)
      .mockResolvedValueOnce(detail({
        id: 'task-internal',
        reference: 'KT-200',
        title: 'Internal blocker',
      }))
      .mockResolvedValueOnce(selectedDetail);
    const onNavigateDiscussion = vi.fn();
    const onNavigateProject = vi.fn();
    const { container } = render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        onNavigateDiscussion={onNavigateDiscussion}
        onNavigateProject={onNavigateProject}
        toast={vi.fn()}
      />,
    );

    await screen.findByText('planning.progress:0/2');
    fireEvent.click(container.querySelector('.plan-task-open') as HTMLElement);
    expect(await screen.findByText('planning.blockedBy')).toBeInTheDocument();
    expect(mocks.get).toHaveBeenCalledTimes(1);
    expect(screen.getByText('planning.blocks')).toBeInTheDocument();

    fireEvent.click(screen.getByLabelText('planning.selectDependency:Internal blocker'));
    await waitFor(() => expect(mocks.get).toHaveBeenLastCalledWith('task-internal'));
    expect(mocks.get).toHaveBeenCalledTimes(2);

    fireEvent.click(container.querySelector('.plan-task-open') as HTMLElement);
    await screen.findByLabelText('planning.openDependencyDiscussion:External blocker');
    fireEvent.click(screen.getByLabelText('planning.openDependencyDiscussion:External blocker'));
    fireEvent.click(screen.getByLabelText('planning.openDependencyProject:External blocker'));
    expect(onNavigateDiscussion).toHaveBeenCalledWith('disc-external');
    expect(onNavigateProject).toHaveBeenCalledWith('project-external');
  });

  it('keeps task mutations available in the selected detail', async () => {
    const { container } = render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );

    await screen.findByText('planning.progress:0/1');
    fireEvent.click(container.querySelector('.plan-task-open') as HTMLElement);
    await screen.findByLabelText('planning.taskActions');
    fireEvent.click(screen.getAllByText('planning.markDone').at(-1) as HTMLElement);
    await waitFor(() => expect(mocks.update).toHaveBeenCalledWith('task-1', {
      status: 'done',
    }));
    fireEvent.click(screen.getByText('planning.removePrimary'));
    await waitFor(() => expect(mocks.linkDiscussion).toHaveBeenCalledWith('task-1', {
      discussion_id: 'disc-1',
      placement: 'active',
      is_primary: false,
    }));
  });

  it('keeps the all-tasks search when switching back to Focus', async () => {
    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );

    await screen.findByText('planning.progress:0/1');
    fireEvent.click(screen.getByText('planning.allView'));
    const search = screen.getByPlaceholderText('planning.allSearchPlaceholder');
    fireEvent.change(search, { target: { value: 'KT-1' } });
    fireEvent.click(screen.getByText('planning.focusView'));
    expect(screen.queryByPlaceholderText('planning.allSearchPlaceholder')).not.toBeInTheDocument();
    fireEvent.click(screen.getByText('planning.allView'));
    expect(screen.getByPlaceholderText('planning.allSearchPlaceholder')).toHaveValue('KT-1');
  });

  it('uses summary chips as reversible filters for the complete plan', async () => {
    const ready = relation({
      id: 'task-ready',
      reference: 'KT-301',
      title: 'Ready task',
    }, {
      actionable: true,
    });
    const blocked = relation({
      id: 'task-blocked',
      reference: 'KT-302',
      title: 'Blocked task',
      status: 'blocked',
    }, {
      position: 1,
      actionable: false,
    });
    const completed = relation({
      id: 'task-done',
      reference: 'KT-303',
      title: 'Completed task',
      status: 'done',
    }, {
      position: 2,
      actionable: false,
    });
    const later = relation({
      id: 'task-later',
      reference: 'KT-304',
      title: 'Later task',
    }, {
      placement: 'later',
      actionable: false,
    });
    mocks.discussionPlan.mockResolvedValue(plan({
      primary_objective: null,
      active: [ready, blocked, completed],
      later: [later],
      total_active: 3,
      completed_active: 1,
      stats: { ready: 1, blocked: 1, in_progress: 0, ideas: 0, done: 1, later: 1 },
    }));

    const { container } = render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );

    const blockedFilter = await screen.findByLabelText('planning.filterBlocked');
    fireEvent.click(blockedFilter);
    expect(blockedFilter).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByPlaceholderText('planning.allSearchPlaceholder')).toBeInTheDocument();
    expect(container.querySelector('.plan-panel-list')).toHaveAttribute('data-view-mode', 'all');
    await waitFor(() => expect(screen.getByRole('listbox')).toHaveAttribute(
      'aria-activedescendant',
      'plan-all-option-task-blocked',
    ));

    fireEvent.click(blockedFilter);
    expect(blockedFilter).toHaveAttribute('aria-pressed', 'false');

    const laterFilter = screen.getByLabelText('planning.filterLater');
    fireEvent.click(laterFilter);
    expect(laterFilter).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByPlaceholderText('planning.allSearchPlaceholder')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('listbox')).toHaveAttribute(
      'aria-activedescendant',
      'plan-all-option-task-later',
    ));
  });

  it('keeps the selected task detail outside the independently scrollable plan list', async () => {
    const { container } = render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );

    await screen.findByText('planning.progress:0/1');
    fireEvent.click(container.querySelector('.plan-task-open') as HTMLElement);
    await screen.findByLabelText('planning.taskActions');

    const body = container.querySelector<HTMLElement>('.plan-panel-body');
    const list = container.querySelector<HTMLElement>('.plan-panel-list');
    const selectedDetail = container.querySelector<HTMLElement>('.plan-detail');
    expect(body).toHaveAttribute('data-detail-open', 'true');
    expect(list?.parentElement).toBe(body);
    expect(selectedDetail?.parentElement).toBe(body);
    expect(list).not.toContainElement(selectedDetail);
  });

  it('shows the durable proposal inbox and accepts one item idempotently', async () => {
    mocks.proposals.mockResolvedValue({
      proposals: [{
        id: 'proposal:message-1:0',
        discussion_id: 'disc-1',
        source_message_id: 'message-1',
        fence_index: 0,
        aggregate_state: 'pending',
        items: [{
          id: 'proposal:message-1:0:item:0',
          item_index: 0,
          action: 'create',
          payload: { title: 'Ship the inbox' },
          state: 'pending',
        }],
        created_at: '2026-07-26T00:00:00Z',
        updated_at: '2026-07-26T00:00:00Z',
      }],
      pending_proposal_count: 1,
      pending_item_count: 1,
    });
    mocks.decideProposalItem.mockResolvedValue({
      item: {
        id: 'proposal:message-1:0:item:0',
        item_index: 0,
        action: 'create',
        payload: { title: 'Ship the inbox' },
        state: 'accepted',
        result_task_id: 'task-new',
      },
      aggregate_state: 'applied',
    });

    render(
      <DiscussionPlanPanel
        discussionId="disc-1"
        onClose={vi.fn()}
        toast={vi.fn()}
      />,
    );

    expect(await screen.findByText('planning.proposalInbox')).toBeInTheDocument();
    expect(screen.getByText('Ship the inbox')).toBeInTheDocument();
    fireEvent.click(screen.getByText('planning.acceptProposal'));
    await waitFor(() => expect(mocks.decideProposalItem).toHaveBeenCalledWith(
      'proposal:message-1:0',
      'proposal:message-1:0:item:0',
      expect.objectContaining({
        decision: 'accept',
        idempotency_key: expect.any(String),
      }),
    ));
  });
});

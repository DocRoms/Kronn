import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createMemoryRouter, RouterProvider, useParams } from 'react-router';

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  get: vi.fn(),
  update: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  planning: {
    ...mocks,
    linkDiscussion: vi.fn(),
    addBlocker: vi.fn(),
    discussionPlan: vi.fn(),
    changes: vi.fn(),
  },
}));

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join('/')}` : key,
  }),
}));

import { PlanningPage } from '../PlanningPage';
import type { PlanningTaskDetail, PlanningTaskSummary } from '../../types/generated';

function summary(overrides: Partial<PlanningTaskSummary> = {}): PlanningTaskSummary {
  return {
    id: 'task-1',
    reference: 'KT-1',
    parent_id: null,
    parent_reference: null,
    parent_title: null,
    title: 'Upgrade PHP',
    status: 'todo',
    priority: 'high',
    rank: 1024,
    completed_subtasks: 0,
    total_subtasks: 0,
    project_ids: [],
    discussion_ids: [],
    tags: ['platform'],
    blocker_count: 0,
    created_at: '2026-07-25T00:00:00Z',
    updated_at: '2026-07-25T00:00:00Z',
    ...overrides,
  };
}

function detail(task: PlanningTaskSummary = summary()): PlanningTaskDetail {
  return {
    ...task,
    subtasks: [],
    description: 'Move the runtime forward.',
    blocked_reason: null,
    definition_of_done: [],
    links: [],
    blockers: [],
    blocking: [],
    events: [],
  };
}

describe('PlanningPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.list.mockResolvedValue({
      items: [
        summary(),
        summary({
          id: 'task-done',
          reference: 'KT-2',
          title: 'Old task',
          status: 'done',
          priority: 'low',
        }),
      ],
      next_cursor: null,
    });
    mocks.create.mockResolvedValue(detail(summary({
      id: 'task-new',
      reference: 'KT-3',
      title: 'New idea',
      status: 'idea',
      priority: 'critical',
    })));
    mocks.get.mockResolvedValue(detail());
    mocks.update.mockImplementation(async (_id, patch) => detail({ ...summary(), ...patch }));
  });

  // Selection is URL-driven (/planning/:taskId), so tests render through a
  // real route: the param loops back into `selectedTaskId` exactly like
  // PlanningRoute does in the app.
  const Harness = (props: Partial<React.ComponentProps<typeof PlanningPage>>) => {
    const { taskId } = useParams<{ taskId?: string }>();
    return (
      <PlanningPage
        selectedTaskId={taskId ?? null}
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
        {...props}
      />
    );
  };
  const wrap = (initialPath = '/planning', props: Partial<React.ComponentProps<typeof PlanningPage>> = {}) => {
    const router = createMemoryRouter(
      [
        { path: '/planning', element: <Harness {...props} /> },
        { path: '/planning/:taskId', element: <Harness {...props} /> },
      ],
      { initialEntries: [initialPath] },
    );
    return { router, ...render(<RouterProvider router={router} />) };
  };

  it('renders active work by priority and keeps completed work collapsed', async () => {
    const { container } = wrap();
    expect(await screen.findByText('Upgrade PHP')).toBeInTheDocument();
    expect(container.querySelector('[data-priority="high"]')).not.toBeNull();
    expect(screen.queryByText('Old task')).toBeNull();
    fireEvent.click(screen.getByText('planning.completed · 1'));
    expect(screen.getByText('Old task')).toBeInTheDocument();
  });

  it('quick creation defaults to an idea and preserves the selected priority', async () => {
    wrap();
    const input = await screen.findByPlaceholderText('planning.newIdea');
    fireEvent.change(input, { target: { value: 'New idea' } });
    const selects = screen.getAllByRole('combobox');
    fireEvent.change(selects[0], { target: { value: 'critical' } });
    fireEvent.keyDown(input, { key: 'Enter' });

    await waitFor(() => expect(mocks.create).toHaveBeenCalledWith({
      title: 'New idea',
      priority: 'critical',
      status: 'idea',
    }));
  });

  it('opens the full detail only after a task is selected', async () => {
    const { router } = wrap();
    fireEvent.click(await screen.findByText('Upgrade PHP'));
    expect(router.state.location.pathname).toBe('/planning/task-1');
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-1'));
    expect(await screen.findByDisplayValue('Move the runtime forward.')).toBeInTheDocument();
  });

  it('closing the detail returns to /planning', async () => {
    const { router, container } = wrap('/planning/task-1');
    await screen.findByDisplayValue('Move the runtime forward.');
    fireEvent.click(container.querySelector('.planning-detail header button[type="button"]:last-child')!);
    expect(router.state.location.pathname).toBe('/planning');
    await waitFor(() => expect(container.querySelector('.planning-detail')).toBeNull());
  });

  it('opens a directly linked task detail on mount', async () => {
    wrap('/planning/task-1');

    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-1'));
    expect(await screen.findByDisplayValue('Move the runtime forward.')).toBeInTheDocument();
  });

  it('creates a subtask from the detail panel with inherited priority and project links', async () => {
    const parent = detail({
      ...summary(),
      project_ids: ['project-1'],
    });
    mocks.get.mockResolvedValue(parent);
    mocks.create.mockResolvedValue(detail(summary({
      id: 'child-1',
      reference: 'KT-2',
      parent_id: 'task-1',
      parent_reference: 'KT-1',
      parent_title: 'Upgrade PHP',
      title: 'Update CI image',
    })));
    wrap();
    fireEvent.click(await screen.findByText('Upgrade PHP'));
    const input = await screen.findByPlaceholderText('planning.addSubtask');
    fireEvent.change(input, { target: { value: 'Update CI image' } });
    fireEvent.keyDown(input, { key: 'Enter' });
    await waitFor(() => expect(mocks.create).toHaveBeenCalledWith({
      title: 'Update CI image',
      status: 'todo',
      priority: 'high',
      parent_id: 'task-1',
      project_ids: ['project-1'],
    }));
  });
});

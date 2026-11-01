import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

const mocks = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  get: vi.fn(),
  update: vi.fn(),
  removeBlocker: vi.fn(),
}));

vi.mock('../../lib/api', () => ({
  planning: {
    ...mocks,
    linkDiscussion: vi.fn(),
    addBlocker: vi.fn(),
    removeBlocker: mocks.removeBlocker,
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
import { MAX_PLANNING_PAGES } from '../../lib/planningPages';
import type { PlanningTaskDetail, PlanningTaskListResponse, PlanningTaskSummary, Project } from '../../types/generated';

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
    assignees: [],
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

/** A list answer whose counts are the ones the backend would report for `items`. */
function listPage(
  items: PlanningTaskSummary[],
  overrides: Partial<PlanningTaskListResponse> = {},
): PlanningTaskListResponse {
  const project_totals: Record<string, number> = {};
  for (const task of items) {
    for (const id of task.project_ids) project_totals[id] = (project_totals[id] ?? 0) + 1;
  }
  return {
    items,
    next_cursor: null,
    total: items.length,
    project_totals,
    unassigned_total: items.filter(task => task.project_ids.length === 0).length,
    ...overrides,
  };
}

async function findCanonicalTaskRow(title: string): Promise<HTMLButtonElement> {
  const matches = await screen.findAllByText(title);
  const match = matches.find(node => node.closest('.disc-sidebar-projects'));
  const row = match?.closest<HTMLButtonElement>('button.collection-shell-row-button');
  if (!row) throw new Error(`Missing canonical Planning row: ${title}`);
  return row;
}

describe('PlanningPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    sessionStorage.clear();
    localStorage.clear();
    vi.stubGlobal('confirm', () => true);
    vi.stubGlobal('matchMedia', vi.fn().mockImplementation((query: string) => ({ matches: false, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn() })));
    mocks.list.mockResolvedValue(listPage([
      summary(),
      summary({
        id: 'task-done',
        reference: 'KT-2',
        title: 'Old task',
        status: 'done',
        priority: 'low',
      }),
    ]));
    mocks.create.mockResolvedValue(detail(summary({
      id: 'task-new',
      reference: 'KT-3',
      title: 'New idea',
      status: 'idea',
      priority: 'critical',
    })));
    mocks.get.mockResolvedValue(detail());
    mocks.update.mockImplementation(async (_id, patch) => detail({ ...summary(), ...patch }));
    mocks.removeBlocker.mockResolvedValue(detail());
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it('renders tasks in the shared collection sidebar', async () => {
    const { container } = render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    expect(await findCanonicalTaskRow('Upgrade PHP')).toBeInTheDocument();
    expect(container.querySelector('.collection-shell-sidebar')).not.toBeNull();
    expect(await findCanonicalTaskRow('Old task')).toBeInTheDocument();
  });

  it('puts the awaiting-merge chip on its own line under the meta row, with the full branch on its title', async () => {
    const branch = 'autocode/campaign/kt-1519-tasks-are-marked-done-too-early';
    mocks.list.mockResolvedValue(listPage([
      summary({ id: 'awaiting', reference: 'KT-1519', title: 'Integrated task', status: 'in_progress', awaiting_merge: { branch, sha: '0123456789abcdef' } }),
    ]));
    const { container } = render(
      <PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />,
    );
    await findCanonicalTaskRow('Integrated task');
    const chip = container.querySelector('.planning-sidebar-awaiting') as HTMLElement;
    expect(chip).not.toBeNull();
    expect(chip.getAttribute('data-status')).toBe('awaiting_merge');
    expect(chip.getAttribute('title')).toBe(`${branch} @ 01234567`);
    expect(chip.textContent).toMatch(/^planning\.status\.awaitingMerge:….*done-too-early$/);
    // Not a child of the meta row (which holds the reference): it follows it, so
    // the reference can never wrap around the chip.
    const meta = container.querySelector('.disc-item-meta') as HTMLElement;
    expect(meta.contains(chip)).toBe(false);
    expect(meta.nextElementSibling).toBe(chip);
    expect(meta.querySelector('.planning-ref')?.textContent).toBe('KT-1519');
    expect(meta.querySelector('.planning-status')).toBeNull();
  });

  it('keeps the plain status pill in the meta row for a task that is not awaiting a merge', async () => {
    const { container } = render(
      <PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />,
    );
    await findCanonicalTaskRow('Upgrade PHP');
    expect(container.querySelector('.planning-sidebar-awaiting')).toBeNull();
    expect(container.querySelector('.disc-item-meta .planning-status')).not.toBeNull();
  });

  it('renders the planning empty state through the real page adapter', async () => {
    mocks.list.mockResolvedValueOnce(listPage([]));
    render(<PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    expect(await screen.findByText('planning.emptyBacklog')).toBeInTheDocument();
  });

  it('covers the narrow rail, empty backlog, selection, and open menu on the real Planning page', async () => {
    vi.stubGlobal('matchMedia', vi.fn().mockImplementation((query: string) => ({ matches: true, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn() })));
    mocks.list.mockResolvedValueOnce(listPage([]));
    const props = { projects: [], discussions: [], toast: vi.fn(), onNavigateDiscussion: vi.fn() };
    const view = render(<PlanningPage {...props} />);
    expect(await screen.findByText('planning.emptyBacklog')).toBeInTheDocument();

    mocks.list.mockReset();
    mocks.list.mockResolvedValue(listPage([summary()]));
    view.unmount();
    render(<PlanningPage {...props} />);
    await findCanonicalTaskRow('Upgrade PHP');
    fireEvent.click(screen.getByRole('button', { name: 'collection.moreActions' }));
    expect(screen.getByRole('menu')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'collection.closeCollection' }));
    expect(screen.getByRole('button', { name: 'collection.openCollection' })).toHaveClass('collection-shell-open');
    fireEvent.click(screen.getByRole('button', { name: 'collection.openCollection' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'collection.selectMultiple' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Upgrade PHP.*collection\.selectItem/ }));
    expect(screen.getByRole('checkbox', { name: /Upgrade PHP.*collection\.selectItem/ })).toBeChecked();
  });

  it('retains a favorite hidden by a filtered paginated response across remounts', async () => {
    const favorite = summary({ id: 'task-later-page', title: 'Later-page favorite' });
    const visible = summary({ id: 'task-visible', title: 'Visible task', status: 'done' });
    localStorage.setItem('kronn:collection-favorites:planning', JSON.stringify([favorite.id]));
    // The saved favorite is not in the filtered answer, whatever the filter.
    mocks.list.mockImplementation(async () => listPage([visible]));

    const props = { projects: [], discussions: [], toast: vi.fn(), onNavigateDiscussion: vi.fn() };
    const first = render(<PlanningPage {...props} />);
    await findCanonicalTaskRow(visible.title);
    expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ cursor: undefined, limit: 100 }));
    fireEvent.change(screen.getByRole('textbox', { name: 'planning.search' }), { target: { value: 'visible' } });
    await waitFor(() => expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({
      search: 'visible',
      limit: 100,
    })));
    expect(JSON.parse(localStorage.getItem('kronn:collection-favorites:planning') ?? '[]')).toEqual([favorite.id]);

    first.unmount();
    render(<PlanningPage {...props} />);
    await findCanonicalTaskRow(visible.title);
    expect(JSON.parse(localStorage.getItem('kronn:collection-favorites:planning') ?? '[]')).toEqual([favorite.id]);
  });

  it('orders favorite and recent shortcuts while keeping the complete project tree', async () => {
    const favorite = summary({
      id: 'favorite-task', reference: 'KT-100', title: 'Favorite task',
      project_ids: ['project-alpha'], updated_at: '2026-08-31T10:00:00Z',
    });
    const recentTasks = Array.from({ length: 11 }, (_, index) => summary({
      id: `recent-${index}`,
      reference: `KT-${index + 101}`,
      title: `Recent ${index}`,
      project_ids: index === 0
        ? ['project-alpha', 'project-beta']
        : index === 1 ? ['project-beta'] : [],
      updated_at: `2026-08-${String(30 - index).padStart(2, '0')}T10:00:00Z`,
    }));
    const projects = [
      { id: 'project-alpha', name: 'Alpha' },
      { id: 'project-beta', name: 'Beta' },
    ] as Project[];
    localStorage.setItem('kronn:collection-favorites:planning', JSON.stringify([favorite.id]));
    mocks.list.mockResolvedValueOnce(listPage([favorite, ...recentTasks]));

    render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    await findCanonicalTaskRow(favorite.title);

    const favoriteSection = screen.getByText('disc.favorites').closest('.disc-sidebar-section') as HTMLElement;
    expect(within(favoriteSection).getByRole('button', { name: 'disc.favorites 1' })).toHaveClass('collection-favorites-header');
    const recentSection = screen.getByText('disc.recent').closest('.disc-sidebar-section') as HTMLElement;
    const projectsSection = screen.getByText('projects.title').closest('.disc-sidebar-section') as HTMLElement;
    expect([...recentSection.querySelectorAll('.disc-item-title-text')].map(node => node.textContent))
      .toEqual(recentTasks.slice(0, 10).map(task => task.title));
    expect(within(recentSection).queryByText(favorite.title)).toBeNull();
    expect(within(favoriteSection).getByText(favorite.title)).toBeInTheDocument();
    expect(within(projectsSection).getAllByText(favorite.title)).toHaveLength(1);

    const alpha = projectsSection.querySelector<HTMLElement>('[data-planning-group="project:project-alpha"]')!;
    const beta = projectsSection.querySelector<HTMLElement>('[data-planning-group="project:project-beta"]')!;
    const general = projectsSection.querySelector<HTMLElement>('[data-planning-group="general"]')!;
    expect(within(alpha).getByText('Recent 0')).toBeInTheDocument();
    expect(within(beta).getByText('Recent 0')).toBeInTheDocument();
    expect(within(general).getByText('Recent 2')).toBeInTheDocument();

    const alphaToggle = within(alpha).getByRole('button', { name: /Alpha/ });
    fireEvent.click(alphaToggle);
    expect(alphaToggle).toHaveAttribute('aria-expanded', 'false');
    expect(within(alpha).queryByText(favorite.title)).toBeNull();
    await waitFor(() => expect(JSON.parse(
      localStorage.getItem('kronn:planningCollapsedSections') ?? '[]',
    )).toContain('project:project-alpha'));

    const betaRecentRow = within(beta).getByText('Recent 0')
      .closest<HTMLButtonElement>('button.collection-shell-row-button')!;
    fireEvent.click(betaRecentRow);
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('recent-0'));
    expect(betaRecentRow).toHaveAttribute('aria-current', 'true');
  });

  it('lists every page, so a project group shows exactly the rows its count announces', async () => {
    // 150 tasks sit in Alpha and a page holds 100: the sidebar pages through
    // both, so the group's count (the backend's, the one the project tab and
    // task_list report) equals the rows it lists.
    const alphaRows = Array.from({ length: 150 }, (_, index) => summary({
      id: `alpha-${index}`, reference: `KT-${index + 10}`, title: `Alpha task ${index}`,
      project_ids: ['project-alpha'],
    }));
    const loose = summary({ id: 'loose', reference: 'KT-500', title: 'Loose task' });
    const everything = [...alphaRows, loose];
    mocks.list.mockImplementation(async filters => {
      const start = filters.cursor ?? 0;
      const end = start + (filters.limit ?? 100);
      // Like the backend: the grouped counts ride on the first page only.
      return listPage(everything.slice(start, end), {
        next_cursor: end < everything.length ? end : null,
        total: everything.length,
        project_totals: start === 0 ? { 'project-alpha': 150 } : {},
        unassigned_total: start === 0 ? 1 : 0,
      });
    });
    const projects = [{ id: 'project-alpha', name: 'Alpha' }] as Project[];

    render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    await findCanonicalTaskRow('Alpha task 149');

    expect(mocks.list).toHaveBeenCalledTimes(2);
    expect(mocks.list).toHaveBeenNthCalledWith(2, expect.objectContaining({ cursor: 100, limit: 100 }));
    const projectsSection = screen.getByText('projects.title').closest('.disc-sidebar-section') as HTMLElement;
    const group = (name: string) => projectsSection.querySelector<HTMLElement>(`[data-planning-group="${name}"]`)!;
    const count = (name: string) => group(name).querySelector('.disc-group-count')?.textContent;
    const listed = (name: string) => group(name).querySelectorAll('button.collection-shell-row-button').length;
    expect(count('project:project-alpha')).toBe('150');
    expect(listed('project:project-alpha')).toBe(150);
    expect(count('general')).toBe('1');
    expect(listed('general')).toBe(1);
    expect(projectsSection.querySelector('.disc-group-btn .disc-group-count')?.textContent).toBe('151');
  });

  it('counts a task linked to a project only through a discussion in that project, not in General', async () => {
    // The backend reports such a task's project in `project_ids`, so it is
    // listed under Alpha; the General count is the backend's, and equals the
    // rows General lists (the one task with no project at all).
    const viaDiscussion = summary({
      id: 'via-discussion', reference: 'KT-20', title: 'Via discussion',
      project_ids: ['project-alpha'], discussion_ids: ['disc-1'],
    });
    const direct = summary({
      id: 'direct', reference: 'KT-21', title: 'Direct task', project_ids: ['project-alpha'],
    });
    const loose = summary({ id: 'loose', reference: 'KT-22', title: 'Loose task' });
    mocks.list.mockResolvedValue(listPage([viaDiscussion, direct, loose], {
      total: 3,
      project_totals: { 'project-alpha': 2 },
      unassigned_total: 1,
    }));
    const projects = [{ id: 'project-alpha', name: 'Alpha' }] as Project[];

    render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    await findCanonicalTaskRow('Via discussion');

    const projectsSection = screen.getByText('projects.title').closest('.disc-sidebar-section') as HTMLElement;
    const group = (name: string) => projectsSection.querySelector<HTMLElement>(`[data-planning-group="${name}"]`)!;
    const count = (name: string) => group(name).querySelector('.disc-group-count')?.textContent;
    const listed = (name: string) => group(name).querySelectorAll('button.collection-shell-row-button').length;
    expect(count('project:project-alpha')).toBe('2');
    expect(listed('project:project-alpha')).toBe(2);
    expect(count('general')).toBe('1');
    expect(listed('general')).toBe(1);
    expect(within(group('general')).queryByText('Via discussion')).toBeNull();
  });

  it('lists, under a search, every task the backend counted, even one matched only by its description', async () => {
    // The backend search also reads the description; the label the shell could
    // filter on (title, reference, tags) does not carry it.
    const descriptions: Record<string, string> = {
      'by-title': 'Nothing relevant',
      'by-description': 'Move the runtime forward',
      other: 'Unrelated',
    };
    const byTitle = summary({
      id: 'by-title', reference: 'KT-30', title: 'Runtime bump', tags: [], project_ids: ['project-alpha'],
    });
    const byDescription = summary({
      id: 'by-description', reference: 'KT-31', title: 'Upgrade PHP', tags: [], project_ids: ['project-alpha'],
    });
    const other = summary({ id: 'other', reference: 'KT-32', title: 'Other', tags: [], project_ids: ['project-alpha'] });
    mocks.list.mockImplementation(async filters => {
      const needle = (filters.search ?? '').toLowerCase();
      const matches = [byTitle, byDescription, other].filter(task => !needle || [
        task.title, task.reference, descriptions[task.id],
      ].some(field => field.toLowerCase().includes(needle)));
      return listPage(matches);
    });
    const projects = [{ id: 'project-alpha', name: 'Alpha' }] as Project[];

    render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    await findCanonicalTaskRow('Other');
    fireEvent.change(screen.getByRole('textbox', { name: 'planning.search' }), { target: { value: 'runtime' } });
    await waitFor(() => expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ search: 'runtime' })));
    await waitFor(() => expect(screen.queryByText('Other')).toBeNull());

    const projectsSection = screen.getByText('projects.title').closest('.disc-sidebar-section') as HTMLElement;
    const alpha = projectsSection.querySelector<HTMLElement>('[data-planning-group="project:project-alpha"]')!;
    expect(alpha.querySelector('.disc-group-count')?.textContent).toBe('2');
    expect(alpha.querySelectorAll('button.collection-shell-row-button')).toHaveLength(2);
    expect(within(alpha).getByText('Upgrade PHP')).toBeInTheDocument();
  });

  it('does not report a failure of a request a newer one superseded', async () => {
    const toast = vi.fn();
    let rejectFirst: (cause: Error) => void = () => {};
    mocks.list.mockReset();
    mocks.list
      .mockImplementationOnce(() => new Promise((_, reject) => { rejectFirst = reject; }))
      .mockResolvedValue(listPage([summary()]));

    render(<PlanningPage projects={[]} discussions={[]} toast={toast} onNavigateDiscussion={vi.fn()} />);
    await waitFor(() => expect(mocks.list).toHaveBeenCalledTimes(1));
    fireEvent.change(screen.getByRole('textbox', { name: 'planning.search' }), { target: { value: 'php' } });
    await findCanonicalTaskRow('Upgrade PHP');
    expect(mocks.list).toHaveBeenCalledTimes(2);

    await act(async () => { rejectFirst(new Error('stale')); });

    expect(toast).not.toHaveBeenCalled();
  });

  it('says so when the page ceiling stops the list short of its count', async () => {
    const everything = Array.from({ length: MAX_PLANNING_PAGES * 100 + 1 }, (_, index) => summary({
      id: `bulk-${index}`, reference: `KT-${index + 10}`, title: `Bulk ${index}`,
    }));
    mocks.list.mockImplementation(async filters => {
      const start = filters.cursor ?? 0;
      const end = start + (filters.limit ?? 100);
      return listPage(everything.slice(start, end), {
        next_cursor: end < everything.length ? end : null,
        total: everything.length,
        project_totals: {},
        unassigned_total: start === 0 ? everything.length : 0,
      });
    });

    render(<PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    expect(await screen.findByText(/^planning\.truncated:/, {}, { timeout: 8000 })).toBeInTheDocument();
    expect(mocks.list).toHaveBeenCalledTimes(MAX_PLANNING_PAGES);
  });

  it('uses the shared title, favorites, collapse, and bulk archive flow', async () => {
    render(<PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    const taskRow = await findCanonicalTaskRow('Upgrade PHP');
    expect(screen.getByText('planning.title')).toHaveTextContent('planning.title · 2');
    fireEvent.click(within(taskRow.closest('.disc-item') as HTMLElement).getByRole('button', { name: /wf\.pin · Upgrade PHP/ }));
    expect(localStorage.getItem('kronn:collection-favorites:planning')).toContain('task-1');
    const favoriteSection = screen.getByText('disc.favorites').closest('.disc-sidebar-section') as HTMLElement;
    expect(within(favoriteSection).getByText('Upgrade PHP')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'collection.moreActions' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'collection.selectMultiple' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Upgrade PHP.*collection\.selectItem/ }));
    fireEvent.click(screen.getByRole('checkbox', { name: /Old task.*collection\.selectItem/ }));
    fireEvent.click(screen.getByRole('button', { name: 'collection.archiveSelected' }));
    await waitFor(() => expect(mocks.update).toHaveBeenCalledTimes(2));
    expect(mocks.update).toHaveBeenCalledWith('task-1', { status: 'archived' });
    expect(mocks.update).toHaveBeenCalledWith('task-done', { status: 'archived' });
    fireEvent.click(screen.getByRole('button', { name: 'collection.closeCollection' }));
    expect(screen.queryByRole('complementary', { name: 'planning.title' })).toBeNull();
    expect(screen.getByRole('button', { name: 'collection.openCollection' })).toHaveClass('collection-shell-sidebar-rail');
  });

  it('uses the shared row menu and keyboard footer', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    const { container } = render(
      <PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />,
    );
    const taskRow = await findCanonicalTaskRow('Upgrade PHP');
    const item = taskRow.closest('.disc-item') as HTMLElement;
    fireEvent.click(within(item).getByRole('button', { name: 'collection.moreActions · Upgrade PHP' }));
    fireEvent.click(screen.getByRole('menuitem', { name: 'disc.copyId' }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('task-1'));
    fireEvent.click(screen.getByRole('menuitem', { name: 'disc.archive' }));
    await waitFor(() => expect(mocks.update).toHaveBeenCalledWith('task-1', { status: 'archived' }));

    const footer = container.querySelector('.disc-sidebar-footer') as HTMLElement;
    expect(footer).toHaveTextContent('planning.sidebar.hint');
    expect(within(footer).getByText('↑↓')).toBeInTheDocument();
    expect(within(footer).getByText('/')).toBeInTheDocument();
  });

  it('keeps Planning task rows in the shared keyboard and active-row contract', async () => {
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    const first = await findCanonicalTaskRow('Upgrade PHP');
    const second = await findCanonicalTaskRow('Old task');
    first.focus();
    fireEvent.keyDown(first, { key: 'ArrowDown' });
    expect(second).toHaveFocus();

    fireEvent.click(second);
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-done'));
    expect(second).toHaveAttribute('aria-current', 'true');
  });

  describe('after a mutation, the rows and the counts are read together', () => {
    /** What the server holds: every list answer is derived from it, as the backend does. */
    function serve(initial: PlanningTaskSummary[]) {
      const store = [...initial];
      mocks.list.mockImplementation(async () => listPage(store.filter(task => task.status !== 'archived')));
      mocks.update.mockImplementation(async (id: string, patch: { status?: PlanningTaskSummary['status'] }) => {
        const task = store.find(item => item.id === id)!;
        Object.assign(task, patch);
        return detail(task);
      });
      return store;
    }
    const projects = [{ id: 'project-alpha', name: 'Alpha' }] as Project[];
    const groupOf = (name: string) => screen.getByText('projects.title')
      .closest('.disc-sidebar-section')!
      .querySelector<HTMLElement>(`[data-planning-group="${name}"]`)!;
    const countOf = (name: string) => groupOf(name).querySelector('.disc-group-count')?.textContent;
    const listedIn = (name: string) => groupOf(name).querySelectorAll('button.collection-shell-row-button').length;
    const totalCount = () => screen.getByText('projects.title')
      .closest('.disc-sidebar-section')!
      .querySelector('.disc-group-count')?.textContent;

    async function createFromDialog(title: string) {
      fireEvent.click(screen.getByRole('button', { name: 'planning.quickCreate' }));
      fireEvent.change(screen.getByRole('textbox', { name: 'planning.taskTitle' }), { target: { value: title } });
      fireEvent.click(screen.getByRole('button', { name: 'planning.createAction' }));
      await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    }

    it('counts a task created through the quick-create dialog', async () => {
      const store = serve([
        summary({ id: 'g-1', reference: 'KT-1', title: 'General one' }),
        summary({ id: 'g-2', reference: 'KT-2', title: 'General two' }),
      ]);
      mocks.create.mockImplementation(async input => {
        const created = summary({ id: 'g-3', reference: 'KT-3', title: input.title, status: 'idea' });
        store.push(created);
        return detail(created);
      });

      render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
      await findCanonicalTaskRow('General one');
      expect(totalCount()).toBe('2');
      expect(countOf('general')).toBe('2');

      await createFromDialog('General three');

      await findCanonicalTaskRow('General three');
      await waitFor(() => expect(totalCount()).toBe('3'));
      expect(countOf('general')).toBe('3');
      expect(listedIn('general')).toBe(3);
    });

    it('keeps the current filters when it re-reads after a creation', async () => {
      const store = serve([summary({ id: 'g-1', reference: 'KT-1', title: 'General one' })]);
      mocks.create.mockImplementation(async input => {
        const created = summary({ id: 'g-2', reference: 'KT-2', title: input.title, status: 'idea' });
        store.push(created);
        return detail(created);
      });

      render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
      await findCanonicalTaskRow('General one');
      fireEvent.change(screen.getByRole('textbox', { name: 'planning.search' }), { target: { value: 'general' } });
      await waitFor(() => expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ search: 'general' })));
      mocks.list.mockClear();

      await createFromDialog('General two');

      await waitFor(() => expect(mocks.list).toHaveBeenCalled());
      expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ search: 'general' }));
    });

    it('counts a subtask created from the detail panel in its parent project', async () => {
      const parent = summary({ id: 'parent', reference: 'KT-1', title: 'Parent task', project_ids: ['project-alpha'] });
      const store = serve([parent]);
      mocks.get.mockImplementation(async id => detail(store.find(task => task.id === id)!));
      mocks.create.mockImplementation(async input => {
        const created = summary({
          id: 'child', reference: 'KT-2', title: input.title, parent_id: 'parent',
          project_ids: input.project_ids ?? [],
        });
        store.push(created);
        return detail(created);
      });

      render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
      fireEvent.click(await findCanonicalTaskRow('Parent task'));
      expect(countOf('project:project-alpha')).toBe('1');
      const input = await screen.findByPlaceholderText('planning.addSubtask');
      fireEvent.change(input, { target: { value: 'Child task' } });
      fireEvent.keyDown(input, { key: 'Enter' });

      await waitFor(() => expect(totalCount()).toBe('2'));
      expect(countOf('project:project-alpha')).toBe('2');
      expect(listedIn('project:project-alpha')).toBe(2);
    });

    describe('a mutation that settles after the filters changed', () => {
      /** A server that filters by title, as the search does, over two fixed tasks. */
      function serveFiltered() {
        const store = [
          summary({ id: 'alpha', reference: 'KT-1', title: 'Alpha task' }),
          summary({ id: 'beta', reference: 'KT-2', title: 'Beta task' }),
        ];
        mocks.list.mockImplementation(async (params: { search?: string }) => listPage(
          store.filter(task => !params.search || task.title.toLowerCase().includes(params.search.toLowerCase())),
        ));
        mocks.get.mockImplementation(async id => detail(store.find(task => task.id === id)!));
        return store;
      }
      const rowTitles = () => [...document.querySelectorAll('.disc-sidebar-projects button.collection-shell-row-button')]
        .map(row => (row.textContent ?? '').match(/(Alpha|Beta|Gamma) task/)?.[0])
        .sort();
      async function searchFor(value: string) {
        fireEvent.change(screen.getByRole('textbox', { name: 'planning.search' }), { target: { value } });
        await waitFor(() => expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ search: value })));
        await waitFor(() => expect(rowTitles()).toEqual(['Beta task']));
      }

      it('keeps the new search when a detail save finishes after it', async () => {
        serveFiltered();
        let finishSave!: () => void;
        mocks.update.mockImplementation((id: string) => new Promise(resolve => {
          finishSave = () => resolve(detail(summary({ id, reference: 'KT-1', title: 'Alpha task' })));
        }));

        render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
        fireEvent.click(await findCanonicalTaskRow('Alpha task'));
        fireEvent.click(await screen.findByRole('button', { name: 'common.save' }));
        await waitFor(() => expect(mocks.update).toHaveBeenCalled());

        await searchFor('beta');
        const readsBefore = mocks.list.mock.calls.length;
        await act(async () => { finishSave(); });

        await waitFor(() => expect(mocks.list.mock.calls.length).toBeGreaterThan(readsBefore));
        expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ search: 'beta' }));
        expect(rowTitles()).toEqual(['Beta task']);
        expect(totalCount()).toBe('1');
      });

      it('keeps the new search when a quick creation finishes after it', async () => {
        const store = serveFiltered();
        let finishCreate!: () => void;
        mocks.create.mockImplementation((input: { title: string }) => new Promise(resolve => {
          finishCreate = () => {
            const created = summary({ id: 'gamma', reference: 'KT-3', title: input.title, status: 'idea' });
            store.push(created);
            resolve(detail(created));
          };
        }));

        render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
        await findCanonicalTaskRow('Alpha task');
        fireEvent.click(screen.getByRole('button', { name: 'planning.quickCreate' }));
        fireEvent.change(screen.getByRole('textbox', { name: 'planning.taskTitle' }), { target: { value: 'Gamma task' } });
        fireEvent.click(screen.getByRole('button', { name: 'planning.createAction' }));
        await waitFor(() => expect(mocks.create).toHaveBeenCalled());

        await searchFor('beta');
        const readsBefore = mocks.list.mock.calls.length;
        await act(async () => { finishCreate(); });

        await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
        await waitFor(() => expect(mocks.list.mock.calls.length).toBeGreaterThan(readsBefore));
        expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ search: 'beta' }));
        expect(rowTitles()).toEqual(['Beta task']);
        expect(totalCount()).toBe('1');
      });
    });

    it('counts a task a bulk archive took out of the backlog', async () => {
      serve([
        summary({ id: 'a-1', reference: 'KT-1', title: 'Keep me', project_ids: ['project-alpha'] }),
        summary({ id: 'a-2', reference: 'KT-2', title: 'Archive me', project_ids: ['project-alpha'] }),
      ]);

      render(<PlanningPage projects={projects} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
      await findCanonicalTaskRow('Archive me');
      expect(countOf('project:project-alpha')).toBe('2');
      fireEvent.click(screen.getByRole('button', { name: 'collection.moreActions' }));
      fireEvent.click(screen.getByRole('menuitem', { name: 'collection.selectMultiple' }));
      fireEvent.click(screen.getByRole('checkbox', { name: /Archive me.*collection\.selectItem/ }));
      fireEvent.click(screen.getByRole('button', { name: 'collection.archiveSelected' }));

      await waitFor(() => expect(countOf('project:project-alpha')).toBe('1'));
      expect(totalCount()).toBe('1');
      expect(listedIn('project:project-alpha')).toBe(1);
    });
  });

  it('opens task creation from the compact header action and removes the inline form', async () => {
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await findCanonicalTaskRow('Upgrade PHP');
    expect(screen.queryByPlaceholderText('planning.newIdea')).toBeNull();

    const trigger = screen.getByRole('button', { name: 'planning.quickCreate' });
    expect(trigger).toHaveClass('collection-shell-icon', 'collection-shell-primary-action');
    fireEvent.click(trigger);

    const dialog = screen.getByRole('dialog', { name: 'planning.createTitle' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    const input = screen.getByRole('textbox', { name: 'planning.taskTitle' });
    expect(input).toHaveFocus();
    fireEvent.change(input, { target: { value: 'New idea' } });
    fireEvent.change(screen.getByRole('combobox', { name: 'planning.priorityField' }), {
      target: { value: 'critical' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'planning.createAction' }));

    await waitFor(() => expect(mocks.create).toHaveBeenCalledWith({
      title: 'New idea',
      priority: 'critical',
      status: 'idea',
    }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(trigger).toHaveFocus();
  });

  it('closes task creation with Escape or the backdrop and restores header focus', async () => {
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await findCanonicalTaskRow('Upgrade PHP');
    const trigger = screen.getByRole('button', { name: 'planning.quickCreate' });

    fireEvent.click(trigger);
    const input = screen.getByRole('textbox', { name: 'planning.taskTitle' });
    fireEvent.change(input, { target: { value: 'Preserved draft' } });
    fireEvent.keyDown(input, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger).toHaveFocus();

    fireEvent.click(trigger);
    expect(screen.getByRole('textbox', { name: 'planning.taskTitle' })).toHaveValue('Preserved draft');
    const backdrop = screen.getByRole('dialog').parentElement;
    expect(backdrop).toHaveClass('planning-create-modal-backdrop');
    fireEvent.mouseDown(backdrop!);
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger).toHaveFocus();
    expect(mocks.create).not.toHaveBeenCalled();
  });

  it('labels filter controls for assistive technology', async () => {
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await findCanonicalTaskRow('Upgrade PHP');

    const search = screen.getByRole('textbox', { name: 'planning.search' });
    fireEvent.keyDown(window, { key: '/' });
    expect(search).toHaveFocus();
    fireEvent.change(search, { target: { value: 'Upgrade' } });
    const searchHeader = search.closest<HTMLElement>('.collection-shell-header')!;
    fireEvent.click(within(searchHeader).getByRole('button', { name: 'collection.clearFilters' }));
    expect(search).toHaveValue('');

    const filterButton = within(searchHeader).getByRole('button', { name: 'planning.filters' });
    expect(filterButton).toHaveClass('collection-shell-search-action', 'collection-shell-search-action-icon');
    expect(filterButton.querySelector('span')).toBeNull();
    expect(filterButton).toHaveAttribute('aria-expanded', 'false');
    expect(filterButton).not.toHaveAttribute('aria-controls');
    expect(screen.queryByRole('combobox', { name: 'planning.allStatuses' })).toBeNull();

    fireEvent.click(filterButton);
    expect(filterButton).toHaveAttribute('aria-expanded', 'true');
    expect(filterButton).toHaveAttribute('aria-controls', 'planning-search-options');
    expect(screen.getByRole('combobox', { name: 'planning.allStatuses' })).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'planning.allProjects' })).toBeInTheDocument();
    expect(screen.getByRole('combobox', { name: 'planning.allLinks' })).toBeInTheDocument();

    fireEvent.change(screen.getByRole('combobox', { name: 'planning.allStatuses' }), { target: { value: 'todo' } });
    expect(filterButton).toHaveAttribute('data-active', 'true');
    expect(filterButton.querySelector('strong')).toBeNull();
    await waitFor(() => expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ status: 'todo' })));
    fireEvent.click(filterButton);
    expect(screen.queryByRole('combobox', { name: 'planning.allStatuses' })).toBeNull();
  });

  it('opens the full detail only after a task is selected', async () => {
    const { container } = render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    fireEvent.click(await findCanonicalTaskRow('Upgrade PHP'));
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-1'));
    expect(await screen.findByText('Move the runtime forward.')).toBeInTheDocument();
    const panel = screen.getByRole('complementary', { name: 'planning.taskActions' });
    expect(panel.querySelector(':scope > header')).toHaveClass('collection-detail-header');
    expect(panel.querySelector('.planning-detail-form')).not.toBeNull();
    expect(container.querySelector('.collection-shell-detail > .planning-detail')).toBe(panel);
  });

  it('renders the description as Markdown and saves a raw edit with the unchanged patch', async () => {
    const task = detail();
    task.description = '# Runtime\n\n| Step | State |\n| --- | --- |\n| PHP | todo |';
    mocks.get.mockResolvedValue(task);
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    fireEvent.click(await findCanonicalTaskRow('Upgrade PHP'));
    const panel = await screen.findByRole('complementary', { name: 'planning.taskActions' });
    expect(await within(panel).findByRole('heading', { level: 1, name: 'Runtime' })).toBeInTheDocument();
    expect(panel.querySelector('.task-description-rendered table')).not.toBeNull();
    expect(within(panel).queryByDisplayValue(/# Runtime/)).toBeNull();

    fireEvent.click(within(panel).getByRole('switch', { name: 'planning.descriptionRawHint' }));
    fireEvent.change(within(panel).getByRole('textbox', { name: 'planning.description' }), {
      target: { value: '## New plan' },
    });
    // Back to the rendered view: the unsaved edit must survive the switch.
    fireEvent.click(within(panel).getByRole('switch'));
    expect(within(panel).getByRole('heading', { level: 2, name: 'New plan' })).toBeInTheDocument();
    fireEvent.click(within(panel).getByText('common.save'));

    await waitFor(() => expect(mocks.update).toHaveBeenCalledWith('task-1', {
      title: 'Upgrade PHP',
      description: '## New plan',
      status: 'todo',
      priority: 'high',
      blocked_reason: null,
      tags: ['platform'],
      project_ids: [],
      definition_of_done: [],
      links: [],
    }));
  });

  it('opens a task row in a new tab on a Ctrl-click, without selecting it', async () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    fireEvent.click(await findCanonicalTaskRow('Upgrade PHP'), { ctrlKey: true });
    expect(open).toHaveBeenCalledWith(`${window.location.origin}/planning/task-1`, '_blank', 'noopener,noreferrer');
    expect(mocks.get).not.toHaveBeenCalled();
    expect(screen.queryByRole('complementary', { name: 'planning.taskActions' })).toBeNull();
    open.mockRestore();
  });

  it('opens a linked discussion of a task in a new tab on a Ctrl-click, without navigating in the app', async () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    mocks.get.mockResolvedValue(detail(summary({ discussion_ids: ['disc-7'] })));
    const onNavigateDiscussion = vi.fn();
    render(
      <PlanningPage
        initialSelectedTaskId="task-1"
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={onNavigateDiscussion}
      />,
    );
    const link = await screen.findByRole('button', { name: 'disc-7' });
    fireEvent.click(link, { ctrlKey: true });
    expect(open).toHaveBeenCalledWith(`${window.location.origin}/discussions/disc-7`, '_blank', 'noopener,noreferrer');
    expect(onNavigateDiscussion).not.toHaveBeenCalled();
    open.mockRestore();
  });

  it('opens the parent, a subtask, a blocker and a workspace of a task in a new tab on a Ctrl-click, without moving in the app', async () => {
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    const child = detail(summary({
      parent_id: 'task-parent',
      parent_reference: 'KT-0',
      parent_title: 'Platform upgrade',
    }));
    child.subtasks = [summary({ id: 'task-sub', reference: 'KT-5', title: 'Bump composer' })];
    child.blockers = [summary({ id: 'task-blocker', reference: 'KT-9', title: 'Dependency' })];
    child.workspaces = [{
      id: 'workspace-3',
      disc_id: 'disc-3',
      branch: 'feature/kt-141',
      state: 'attached',
      ownership: 'external',
      session_agent_type: 'Codex',
    }];
    mocks.get.mockResolvedValue(child);
    const onNavigateDiscussion = vi.fn();
    const { container } = render(
      <PlanningPage
        initialSelectedTaskId="task-1"
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={onNavigateDiscussion}
      />,
    );
    const workspace = (await screen.findByText('feature/kt-141')).closest('button') as HTMLElement;
    const loads = mocks.get.mock.calls.length;

    fireEvent.click(container.querySelector('.planning-parent-link') as HTMLElement, { ctrlKey: true });
    fireEvent.click(screen.getByText('Bump composer').closest('button') as HTMLElement, { ctrlKey: true });
    fireEvent.click(container.querySelector('.planning-blocker-row button') as HTMLElement, { ctrlKey: true });
    fireEvent.click(workspace, { ctrlKey: true });

    expect(open.mock.calls).toEqual([
      [`${window.location.origin}/planning/task-parent`, '_blank', 'noopener,noreferrer'],
      [`${window.location.origin}/planning/task-sub`, '_blank', 'noopener,noreferrer'],
      [`${window.location.origin}/planning/task-blocker`, '_blank', 'noopener,noreferrer'],
      [`${window.location.origin}/discussions/disc-3`, '_blank', 'noopener,noreferrer'],
    ]);
    expect(mocks.get).toHaveBeenCalledTimes(loads);
    expect(onNavigateDiscussion).not.toHaveBeenCalled();
    open.mockRestore();
  });

  it('opens a directly linked task detail on mount', async () => {
    render(
      <PlanningPage
        initialSelectedTaskId="task-1"
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-1'));
    expect(await screen.findByText('Move the runtime forward.')).toBeInTheDocument();
  });

  it('keeps a valid initial selection that is outside the first loaded task page', async () => {
    const outsideFirstPage = summary({
      id: 'task-outside-first-page',
      reference: 'KT-101',
      title: 'Outside first page',
    });
    mocks.list.mockResolvedValueOnce(listPage([summary()]));
    mocks.get.mockResolvedValueOnce(detail(outsideFirstPage));
    render(
      <PlanningPage
        initialSelectedTaskId={outsideFirstPage.id}
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await findCanonicalTaskRow('Upgrade PHP');
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith(outsideFirstPage.id));
    expect(await screen.findByText('Move the runtime forward.')).toBeInTheDocument();
    expect(screen.getByRole('complementary', { name: 'planning.taskActions' })).toBeInTheDocument();
  });

  it('reports every selection to the owner of the address, and follows what it is given', async () => {
    const onSelectedTaskChange = vi.fn();
    const view = render(
      <PlanningPage
        selectedTaskId={null}
        onSelectedTaskChange={onSelectedTaskChange}
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    const row = await findCanonicalTaskRow('Upgrade PHP');

    fireEvent.click(row);

    // Controlled: the page asks, and shows nothing until it is given the task.
    expect(onSelectedTaskChange).toHaveBeenCalledWith('task-1');
    expect(screen.getByText('planning.selectHint')).toBeInTheDocument();
    expect(mocks.get).not.toHaveBeenCalled();

    view.rerender(
      <PlanningPage
        selectedTaskId="task-1"
        onSelectedTaskChange={onSelectedTaskChange}
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('task-1'));
    expect(await screen.findByText('Move the runtime forward.')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'common.close' }));
    expect(onSelectedTaskChange).toHaveBeenLastCalledWith(null);
  });

  it('shows the task the address names, never the previous one while it loads', async () => {
    const first = detail(summary());
    const second = detail(summary({ id: 'task-2', reference: 'KT-2', title: 'Second' }));
    second.description = 'The second description.';
    let releaseSecond!: () => void;
    mocks.get.mockImplementation((id: string) => id === 'task-1'
      ? Promise.resolve(first)
      : new Promise(resolve => { releaseSecond = () => resolve(second); }));
    const props = { onSelectedTaskChange: vi.fn(), projects: [], discussions: [], toast: vi.fn(), onNavigateDiscussion: vi.fn() };
    const view = render(<PlanningPage selectedTaskId="task-1" {...props} />);
    expect(await screen.findByText('Move the runtime forward.')).toBeInTheDocument();

    // Back, a link: the address changes without a click in the page.
    view.rerender(<PlanningPage selectedTaskId="task-2" {...props} />);

    expect(screen.queryByText('Move the runtime forward.')).toBeNull();
    expect(screen.getByRole('complementary', { name: 'planning.taskActions' }).querySelector('.planning-state')).not.toBeNull();
    await act(async () => { releaseSecond(); });
    expect(await screen.findByText('The second description.')).toBeInTheDocument();
    expect(props.onSelectedTaskChange).not.toHaveBeenCalled();
  });

  it('clears a directly linked selection when its detail cannot be loaded', async () => {
    const toast = vi.fn();
    mocks.get.mockRejectedValueOnce(new Error('Task no longer exists'));
    render(
      <PlanningPage
        initialSelectedTaskId="missing-task"
        projects={[]}
        discussions={[]}
        toast={toast}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    await waitFor(() => expect(mocks.get).toHaveBeenCalledWith('missing-task'));
    expect(await screen.findByText('planning.selectHint')).toBeInTheDocument();
    expect(screen.queryByRole('complementary', { name: 'planning.taskActions' })).toBeNull();
    expect(toast).toHaveBeenCalled();
  });

  it('opens a linked discussion with the exact workspace selected for its Git panel', async () => {
    const linked = detail();
    linked.workspaces = [{
      id: 'workspace-2',
      disc_id: 'disc-2',
      branch: 'feature/kt-140',
      state: 'attached',
      ownership: 'external',
      session_agent_type: 'Codex',
    }];
    mocks.get.mockResolvedValue(linked);
    const onNavigateDiscussion = vi.fn();
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={onNavigateDiscussion}
      />,
    );

    fireEvent.click(await findCanonicalTaskRow('Upgrade PHP'));
    fireEvent.click(await screen.findByText('feature/kt-140'));

    expect(onNavigateDiscussion).toHaveBeenCalledWith('disc-2', { gitWorkspaceId: 'workspace-2' });
    expect(sessionStorage.getItem('kronn:discussion-workspace-target')).toBeNull();
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
    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );
    fireEvent.click(await findCanonicalTaskRow('Upgrade PHP'));
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

  it('offers to unblock a task when every blocker is done or archived', async () => {
    const blocked = detail(summary({ status: 'blocked', blocker_count: 0 }));
    blocked.blocked_reason = 'Waiting for the old migration.';
    blocked.blockers = [
      summary({
        id: 'blocker-archived',
        reference: 'KT-9',
        title: 'Legacy migration',
        status: 'archived',
      }),
    ];
    mocks.get.mockResolvedValue(blocked);

    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    fireEvent.click(await findCanonicalTaskRow('Upgrade PHP'));
    fireEvent.click(await screen.findByText('planning.unblock'));

    await waitFor(() => expect(mocks.update).toHaveBeenCalledWith('task-1', {
      status: 'todo',
      blocked_reason: null,
    }));
  });

  it('removes one blocker without changing the task status', async () => {
    const blocked = detail(summary({ status: 'blocked', blocker_count: 1 }));
    blocked.blockers = [summary({
      id: 'blocker-9',
      reference: 'KT-9',
      title: 'Dependency',
      status: 'todo',
    })];
    mocks.get.mockResolvedValue(blocked);
    mocks.removeBlocker.mockResolvedValue(detail(summary({
      status: 'blocked',
      blocker_count: 0,
    })));

    render(
      <PlanningPage
        projects={[]}
        discussions={[]}
        toast={vi.fn()}
        onNavigateDiscussion={vi.fn()}
      />,
    );

    fireEvent.click(await findCanonicalTaskRow('Upgrade PHP'));
    fireEvent.click(await screen.findByRole('button', {
      name: 'planning.removeBlocker:KT-9',
    }));

    await waitFor(() => expect(mocks.removeBlocker).toHaveBeenCalledWith(
      'task-1',
      'blocker-9',
    ));
    expect(mocks.update).not.toHaveBeenCalled();
  });

  // KT-1148 — the first load does not wait out the filter debounce, and the
  // sidebar shows the shared loader rather than an empty backlog meanwhile.
  it('starts the first load at once and shows the loader until it answers', async () => {
    let answer: (value: PlanningTaskListResponse) => void = () => {};
    mocks.list.mockReturnValueOnce(new Promise(resolve => { answer = resolve; }));
    render(
      <PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />,
    );
    expect(mocks.list).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('status')).toHaveTextContent('common.loading');
    expect(screen.queryByText('planning.emptyBacklog')).toBeNull();

    answer(listPage([]));
    expect(await screen.findByText('planning.emptyBacklog')).toBeInTheDocument();
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('drops the loader when the first load fails', async () => {
    const toast = vi.fn();
    mocks.list.mockRejectedValueOnce(new Error('down'));
    render(<PlanningPage projects={[]} discussions={[]} toast={toast} onNavigateDiscussion={vi.fn()} />);
    expect(await screen.findByText('planning.emptyBacklog')).toBeInTheDocument();
    expect(toast).toHaveBeenCalledWith(expect.any(String), 'error');
  });

  it('still debounces the loads that follow a filter change', async () => {
    render(<PlanningPage projects={[]} discussions={[]} toast={vi.fn()} onNavigateDiscussion={vi.fn()} />);
    expect(await findCanonicalTaskRow('Upgrade PHP')).toBeInTheDocument();
    expect(mocks.list).toHaveBeenCalledTimes(1);
    vi.useFakeTimers();
    try {
      fireEvent.change(screen.getByRole('textbox', { name: 'planning.search' }), { target: { value: 'php' } });
      expect(mocks.list).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(200);
      expect(mocks.list).toHaveBeenCalledTimes(2);
      expect(mocks.list).toHaveBeenLastCalledWith(expect.objectContaining({ search: 'php' }));
    } finally {
      vi.useRealTimers();
    }
  });
});

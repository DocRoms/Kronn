import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { withDashboardRoutes } from '../../test/routerWrapper';
import type { DiscussionListItem } from '../../types/generated';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));

vi.mock('../DiscussionsPage', () => ({
  DiscussionsPage: ({
    initialActiveDiscussionId,
    initialMessageId,
  }: {
    initialActiveDiscussionId?: string | null;
    initialMessageId?: string | null;
  }) => (
    <div data-testid="discussion-page" data-message-id={initialMessageId}>
      {initialActiveDiscussionId ?? 'discussion-list'}
    </div>
  ),
}));

vi.mock('../McpPage', () => ({ McpPage: () => <div data-testid="mcp-page" /> }));
vi.mock('../WorkflowsPage', () => ({ WorkflowsPage: () => <div data-testid="workflow-page" /> }));
vi.mock('../PlanningPage', () => ({ PlanningPage: () => <div data-testid="planning-page" /> }));
vi.mock('../SettingsPage', () => ({
  SettingsPage: ({ embedOriginPrefill }: { embedOriginPrefill?: { origin: string } | null }) => (
    <div data-testid="settings-page" data-prefill={embedOriginPrefill?.origin ?? ''} />
  ),
}));
vi.mock('../PagesPage', () => ({ PagesPage: () => <div data-testid="pages-page" /> }));

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { activity as activityApi, discussions as discussionsApi, pages as pagesApi, workflows as workflowsApi, projects as projectsApi } from '../../lib/api';
import { Dashboard } from '../Dashboard';
import { navigateAppTab } from '../../lib/live-page-navigation';

const makeDiscussion = (id: string): DiscussionListItem => ({
  // KT-595 — the list carries the pending-decision count now.
  pending_question_count: 0,
  id,
  project_id: null,
  title: `Discussion ${id}`,
  agent: 'Codex',
  language: 'fr',
  participants: ['Codex'],
  messages: [],
  message_count: 0,
  non_system_message_count: 0,
  archived: false,
  pinned: false,
  pin_first_message: false,
  tier: 'default',
  summary_strategy: 'OnDemand',
  introspection_call_count: 0,
  workspace_mode: 'Direct',
  created_at: '2026-07-28T10:00:00Z',
  updated_at: '2026-07-28T10:00:00Z',
  awaiting_agent: false,
});

async function renderDashboard(initialPath = '/') {
  await act(async () => {
    render(
      <I18nProvider>
        {withDashboardRoutes(<Dashboard onReset={vi.fn()} />, initialPath)}
      </I18nProvider>,
    );
  });
}

const navTab = (page: string) => {
  const tab = document.querySelector<HTMLAnchorElement>(`[data-tour-id="nav-${page}"]`);
  if (!tab) throw new Error(`No nav tab for ${page}`);
  return tab;
};

beforeEach(() => {
  sessionStorage.clear();
  window.location.hash = '';
  vi.mocked(discussionsApi.list).mockResolvedValue([]);
  vi.mocked(workflowsApi.list).mockResolvedValue([]);
  vi.mocked(projectsApi.auditStatusAll).mockResolvedValue([]);
  vi.mocked(activityApi.overview).mockResolvedValue({ rows: [], stall_after_secs: 900, generated_at: '2026-11-10T12:00:00Z', unavailable_sources: [] });
  vi.mocked(pagesApi.capability).mockResolvedValue({ activated: false, activated_at: null });
});

afterEach(() => {
  cleanup();
  sessionStorage.clear();
  vi.clearAllMocks();
});

describe('Dashboard — the permanent activity bell (KT-1658)', () => {
  const pausedRun = {
    id: 'workflow_runs:run-paused', type: 'workflow_run', compartment: 'running' as const, state: 'paused' as const,
    source: 'workflow_runs', title: 'PR Review v3.4', detail: 'Agent', started_at: '2026-11-09T16:19:00Z',
    stalled: false, wake_at: '2026-11-11T02:02:00Z',
    link: { target: 'workflow' as const, id: 'wf-review', run_id: 'run-paused' },
    actions: [{ id: 'resume_workflow_run', kind: 'resume_now' as const, args: { run_id: 'run-paused' } }],
  };
  const turn = {
    id: 'agent_dispatch_jobs:j1', type: 'agent_turn', compartment: 'running' as const, state: 'running' as const,
    source: 'agent_dispatch_jobs', title: 'Fix the login', started_at: '2026-11-10T11:56:00Z', stalled: false,
    link: { target: 'discussion' as const, id: 'disc-running' },
    actions: [{ id: 'stop_dispatch', kind: 'stop' as const, args: { discussion_id: 'disc-running', dispatch_id: 'j1' } }],
  };
  const serve = (rows: unknown[]) => vi.mocked(activityApi.overview).mockResolvedValue({
    rows, stall_after_secs: 900, generated_at: '2026-11-10T12:00:00Z', unavailable_sources: [],
  } as never);

  it('is the one activity entry in the header: no spinner next to Config, no "N en cours" text, no popover triggers', async () => {
    vi.mocked(workflowsApi.list).mockResolvedValue([{
      id: 'wf-live', name: 'Live workflow', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0, unsafe_step_count: 0, enabled: true, pinned: false,
      last_run: { id: 'run-live', status: 'Running', started_at: '2026-01-01T00:00:00Z', finished_at: null, tokens_used: 0 },
      created_at: '2026-01-01T00:00:00Z',
    }]);
    vi.mocked(projectsApi.auditStatusAll).mockResolvedValue([{
      project_id: 'project-live', phase: 'auditing', step_index: 1, total_steps: 2,
      current_file: null, started_at: '2026-01-01T00:00:00Z', kind: 'full_audit',
    }]);
    const api = discussionsApi as unknown as { getRunning?: () => Promise<string[]> };
    api.getRunning = vi.fn().mockResolvedValue(['disc-running']);
    try {
      serve([turn]);
      await renderDashboard('/discussions');
      await act(async () => { await new Promise(r => setTimeout(r, 0)); });

      expect(screen.getAllByTestId('activity-bell')).toHaveLength(1);
      expect(screen.queryByTestId('active-runs-trigger')).toBeNull();
      expect(screen.queryByTestId('active-audits-trigger')).toBeNull();
      expect(document.querySelector('.dash-running-badge')).toBeNull();
      expect(screen.queryByText(/en cours$/)).toBeNull();
      // The bell sits in the header's activity group, after the page tabs.
      const group = document.querySelector('.dash-nav-activity-group');
      expect(group?.contains(screen.getByTestId('activity-bell'))).toBe(true);
      // The tabs themselves still navigate and keep their passive counts.
      expect(document.querySelector('[data-tour-id="nav-projects"]')?.querySelector('button')).toBeNull();
      expect(document.querySelector('[data-tour-id="nav-workflows"]')?.querySelector('button')).toBeNull();
    } finally {
      delete api.getRunning;
    }
  });

  it('is there with nothing running, and moves once something does', async () => {
    serve([]);
    await renderDashboard('/discussions');
    expect(screen.getByTestId('activity-bell')).toHaveAttribute('data-running', 'false');
    cleanup();

    serve([turn]);
    await renderDashboard('/discussions');
    expect(await screen.findByTestId('activity-bell')).toHaveAttribute('data-running', 'true');
  });

  it('lists a quota-paused run in the panel with its wake time and a Resume now, and does not count it as running', async () => {
    serve([pausedRun]);
    vi.mocked(workflowsApi.resumeRun).mockResolvedValue({ run_id: 'run-paused', new_status: 'Running' });
    await renderDashboard('/discussions');

    const bell = screen.getByTestId('activity-bell');
    expect(bell).toHaveAttribute('data-running', 'false');
    await act(async () => { fireEvent.click(bell); });

    const item = await screen.findByTestId('activity-row');
    expect(item).toHaveTextContent('PR Review v3.4');
    expect(item).toHaveTextContent('En pause');
    expect(item).toHaveTextContent('Reprise automatique à');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Reprendre maintenant/ })); });
    expect(workflowsApi.resumeRun).toHaveBeenCalledWith('run-paused');
  });

  it('stops a discussion agent from the panel with the existing call, then refreshes the workflow list', async () => {
    serve([turn]);
    vi.mocked(discussionsApi.stopDispatch).mockResolvedValue({ cancelled: true, dispatch_id: 'j1', still_awaiting: false });
    await renderDashboard('/discussions');
    await act(async () => { fireEvent.click(screen.getByTestId('activity-bell')); });
    const before = vi.mocked(workflowsApi.list).mock.calls.length;

    await act(async () => { fireEvent.click(await screen.findByRole('button', { name: /Arrêter : Fix the login/ })); });
    expect(discussionsApi.stopDispatch).toHaveBeenCalledWith('disc-running', 'j1');
    expect(vi.mocked(workflowsApi.list).mock.calls.length).toBeGreaterThan(before);
  });

  it('opens Discussions on the row\'s discussion in place on a plain click, and in a new tab on a Ctrl-click', async () => {
    serve([turn]);
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    try {
      await renderDashboard('/projects');
      await act(async () => { fireEvent.click(screen.getByTestId('activity-bell')); });
      const link = await screen.findByRole('button', { name: 'Ouvrir Fix the login' });

      fireEvent.click(link, { ctrlKey: true });
      expect(open).toHaveBeenCalledWith(`${window.location.origin}/discussions/disc-running`, '_blank', 'noopener,noreferrer');
      expect(screen.queryByTestId('discussion-page')).toBeNull();

      await act(async () => { fireEvent.click(link); });
      expect(await screen.findByTestId('discussion-page')).toHaveTextContent('disc-running');
      expect(window.location.pathname).toBe('/discussions/disc-running');
      expect(screen.queryByRole('dialog', { name: 'Activité' })).toBeNull();
      expect(open).toHaveBeenCalledTimes(1);
    } finally {
      open.mockRestore();
    }
  });
});

describe('Dashboard navigation', () => {
  it('always navigates Projects and Automation: their counts stay passive badges', async () => {
    vi.mocked(workflowsApi.list).mockResolvedValue([{
      id: 'wf-live', name: 'Live workflow', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0, unsafe_step_count: 0, enabled: true, pinned: false,
      last_run: { id: 'run-live', status: 'Running', started_at: '2026-01-01T00:00:00Z', finished_at: null, tokens_used: 0 },
      created_at: '2026-01-01T00:00:00Z',
    }]);
    vi.mocked(projectsApi.auditStatusAll).mockResolvedValue([{
      project_id: 'project-live', phase: 'auditing', step_index: 1, total_steps: 2,
      current_file: null, started_at: '2026-01-01T00:00:00Z', kind: 'full_audit',
    }]);
    await renderDashboard();
    const projectsTab = document.querySelector<HTMLAnchorElement>('[data-tour-id="nav-projects"]');
    const workflowsTab = document.querySelector<HTMLAnchorElement>('[data-tour-id="nav-workflows"]');
    expect(projectsTab).not.toBeNull();
    expect(workflowsTab).not.toBeNull();
    expect(projectsTab?.querySelector('.dash-nav-badge')).toHaveTextContent('1');
    expect(workflowsTab?.querySelector('.dash-nav-badge')).toHaveTextContent('1');
    expect(projectsTab?.querySelector('button')).toBeNull();
    expect(workflowsTab?.querySelector('button')).toBeNull();
    await act(async () => { workflowsTab?.click(); });
    expect(await screen.findByTestId('workflow-page')).toBeInTheDocument();
    await act(async () => { projectsTab?.click(); });
    expect(projectsTab).toHaveAttribute('aria-current', 'page');
  });
  it('reveals Artifacts only after the first Artifact has activated the capability', async () => {
    await renderDashboard();
    expect(screen.queryByRole('link', { name: 'Artifacts' })).not.toBeInTheDocument();
    cleanup();

    vi.mocked(pagesApi.capability).mockResolvedValue({
      activated: true,
      activated_at: '2026-08-13T10:00:00Z',
    });
    await renderDashboard();

    const button = await screen.findByRole('link', { name: 'Artifacts' });
    const navOrder = Array.from(
      document.querySelectorAll<HTMLAnchorElement>('.dash-nav-tabs [data-tour-id^="nav-"]'),
      item => item.dataset.tourId,
    );
    expect(navOrder).toEqual([
      'nav-projects',
      'nav-discussions',
      'nav-planning',
      'nav-workflows',
      'nav-pages',
      'nav-mcps',
      'nav-settings',
    ]);
    button.click();
    expect(await screen.findByTestId('pages-page')).toBeInTheDocument();
  });

  it('reveals Artifacts immediately when a workflow import activates the capability', async () => {
    vi.mocked(pagesApi.capability)
      .mockResolvedValueOnce({ activated: false, activated_at: null })
      .mockResolvedValueOnce({ activated: true, activated_at: '2026-08-26T16:00:00Z' });
    await renderDashboard();
    expect(screen.queryByRole('link', { name: 'Artifacts' })).not.toBeInTheDocument();

    await act(async () => {
      window.dispatchEvent(new Event('kronn:pages-activated'));
    });

    const pagesButton = await screen.findByRole('link', { name: 'Artifacts' });
    expect(pagesApi.capability).toHaveBeenCalledTimes(2);
    pagesButton.click();
    expect(await screen.findByTestId('pages-page')).toBeInTheDocument();
  });

  it('restores the Discussions page and its existing active discussion', async () => {
    sessionStorage.setItem('kronn:navigation:discussion', 'disc-42');
    vi.mocked(discussionsApi.list).mockResolvedValue([makeDiscussion('disc-42')]);

    await renderDashboard('/discussions');

    expect(await screen.findByTestId('discussion-page')).toHaveTextContent('disc-42');
    expect(discussionsApi.runAgent).not.toHaveBeenCalled();
    expect(discussionsApi.sendMessageStream).not.toHaveBeenCalled();
  });

  it('opens a discussion named by the address even when the list has not loaded it', async () => {
    // KT-552 — the checkpoint is filtered against the loaded list, because it
    // may point at a discussion since deleted. An ADDRESS must not be: the
    // list is paginated and loads asynchronously, so filtering it refused to
    // open a discussion merely absent from the first page — or created a
    // second ago. The page fetches the target by id anyway.
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await renderDashboard('/discussions/disc-deep?message=message-origin');

    expect(await screen.findByTestId('discussion-page')).toHaveTextContent('disc-deep');
    expect(screen.getByTestId('discussion-page')).toHaveAttribute('data-message-id', 'message-origin');
    expect(window.location.pathname).toBe('/discussions/disc-deep');
  });

  it('prefers the discussion the address names over the checkpoint', async () => {
    sessionStorage.setItem('kronn:navigation:discussion', 'disc-42');
    vi.mocked(discussionsApi.list).mockResolvedValue([makeDiscussion('disc-42'), makeDiscussion('disc-7')]);

    await renderDashboard('/discussions/disc-7');

    expect(await screen.findByTestId('discussion-page')).toHaveTextContent('disc-7');
  });

  it('does nothing on the tab of the page already open: its address and history stay', async () => {
    await renderDashboard('/discussions/disc-deep');
    expect(await screen.findByTestId('discussion-page')).toHaveTextContent('disc-deep');
    const depth = window.history.length;

    await act(async () => { navTab('discussions').click(); });

    expect(window.location.pathname).toBe('/discussions/disc-deep');
    expect(window.history.length).toBe(depth);
    expect(screen.getByTestId('discussion-page')).toHaveTextContent('disc-deep');
  });

  it('lets the reader leave a discussion address through the nav', async () => {
    await renderDashboard('/discussions/disc-deep');
    expect(await screen.findByTestId('discussion-page')).toBeInTheDocument();

    await act(async () => { navTab('planning').click(); });

    expect(await screen.findByTestId('planning-page')).toBeInTheDocument();
    expect(window.location.pathname).toBe('/planning');
  });

  it('opens Artifacts settings with a blocked site typed in, on load and from a link in place', async () => {
    await renderDashboard('/config/artifacts?origin=https%3A%2F%2Fvimeo.com');
    expect(await screen.findByTestId('settings-page')).toHaveAttribute('data-prefill', 'https://vimeo.com');
    // Consumed: a reload must not prefill it again.
    await waitFor(() => expect(window.location.pathname).toBe('/config'));
    expect(window.location.search).toBe('');

    await act(async () => {
      navigateAppTab('/config/artifacts?origin=https%3A%2F%2Fplayer.example.com');
    });
    await waitFor(() => expect(screen.getByTestId('settings-page')).toHaveAttribute('data-prefill', 'https://player.example.com'));
    await waitFor(() => expect(window.location.pathname).toBe('/config'));
  });

  it('drops a stale discussion id and keeps the safe list view', async () => {
    sessionStorage.setItem('kronn:navigation:discussion', 'deleted-disc');
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await renderDashboard('/discussions');

    expect(await screen.findByTestId('discussion-page')).toHaveTextContent('discussion-list');
    await waitFor(() => {
      expect(sessionStorage.getItem('kronn:navigation:discussion')).toBeNull();
    });
  });
});

describe('Dashboard page addresses', () => {
  const PAGES = [
    ['discussions', '/discussions', 'discussion-page'],
    ['planning', '/planning', 'planning-page'],
    ['workflows', '/workflows', 'workflow-page'],
    ['mcps', '/plugins', 'mcp-page'],
    ['settings', '/config', 'settings-page'],
  ] as const;

  it('makes every tab a link to its page, so a modified click opens it in a new tab', async () => {
    await renderDashboard('/projects');
    expect(navTab('discussions')).toHaveAttribute('href', '/discussions');
    expect(navTab('settings')).toHaveAttribute('href', '/config');
    expect(navTab('mcps')).toHaveAttribute('href', '/plugins');

    // Ctrl/Cmd/Shift-click or a middle click: left to the browser (new tab /
    // window), so the app does not navigate.
    for (const init of [{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { button: 1 }]) {
      const click = new MouseEvent('click', { bubbles: true, cancelable: true, ...init });
      act(() => { navTab('discussions').dispatchEvent(click); });
      expect(click.defaultPrevented).toBe(false);
      expect(screen.queryByTestId('discussion-page')).toBeNull();
      expect(navTab('projects')).toHaveAttribute('aria-current', 'page');
    }

    // A plain click stays in the app.
    const plain = new MouseEvent('click', { bubbles: true, cancelable: true });
    await act(async () => { navTab('discussions').dispatchEvent(plain); });
    expect(plain.defaultPrevented).toBe(true);
    expect(await screen.findByTestId('discussion-page')).toBeInTheDocument();
    expect(window.location.pathname).toBe('/discussions');
  });

  it('lands on Projects from the bare address', async () => {
    await renderDashboard('/');

    expect(window.location.pathname).toBe('/projects');
    expect(navTab('projects')).toHaveAttribute('aria-current', 'page');
  });

  it('lands on Projects from an address that names no page', async () => {
    await renderDashboard('/nowhere/at-all');

    expect(window.location.pathname).toBe('/projects');
    expect(navTab('projects')).toHaveAttribute('aria-current', 'page');
  });

  it.each(PAGES)('opens %s straight from its address', async (page, path, testId) => {
    await renderDashboard(path);

    expect(await screen.findByTestId(testId)).toBeInTheDocument();
    expect(navTab(page)).toHaveAttribute('aria-current', 'page');
    expect(navTab('projects')).not.toHaveAttribute('aria-current');
    expect(window.location.pathname).toBe(path);
  });

  it.each(PAGES)('moves the address to %s when its tab is clicked', async (page, path, testId) => {
    await renderDashboard('/projects');

    await act(async () => { navTab(page).click(); });

    expect(await screen.findByTestId(testId)).toBeInTheDocument();
    expect(window.location.pathname).toBe(path);
  });

  it('keeps one page on screen at a time', async () => {
    await renderDashboard('/planning');
    expect(await screen.findByTestId('planning-page')).toBeInTheDocument();

    await act(async () => { navTab('settings').click(); });

    expect(await screen.findByTestId('settings-page')).toBeInTheDocument();
    expect(screen.queryByTestId('planning-page')).not.toBeInTheDocument();
  });

  it('follows the browser back and forward buttons', async () => {
    await renderDashboard('/projects');
    await act(async () => { navTab('planning').click(); });
    await act(async () => { navTab('workflows').click(); });
    expect(await screen.findByTestId('workflow-page')).toBeInTheDocument();

    await act(async () => { window.history.back(); });
    expect(await screen.findByTestId('planning-page')).toBeInTheDocument();
    expect(navTab('planning')).toHaveAttribute('aria-current', 'page');

    await act(async () => { window.history.forward(); });
    expect(await screen.findByTestId('workflow-page')).toBeInTheDocument();
    expect(window.location.pathname).toBe('/workflows');
  });

  it('refuses the Artifacts address until the capability is activated', async () => {
    await renderDashboard('/pages');

    await waitFor(() => expect(window.location.pathname).toBe('/projects'));
    expect(screen.queryByTestId('pages-page')).not.toBeInTheDocument();
  });

  it('opens Artifacts from its address once the capability is activated', async () => {
    vi.mocked(pagesApi.capability).mockResolvedValue({ activated: true, activated_at: '2026-08-13T10:00:00Z' });

    await renderDashboard('/pages');

    expect(await screen.findByTestId('pages-page')).toBeInTheDocument();
    expect(window.location.pathname).toBe('/pages');
  });
});

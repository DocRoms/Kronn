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

import { discussions as discussionsApi, pages as pagesApi, workflows as workflowsApi, projects as projectsApi } from '../../lib/api';
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
  vi.mocked(pagesApi.capability).mockResolvedValue({ activated: false, activated_at: null });
});

afterEach(() => {
  cleanup();
  sessionStorage.clear();
  vi.clearAllMocks();
});

describe('Dashboard navigation', () => {
  it('always navigates Projects and Automation while activity disclosures remain separate buttons', async () => {
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
    expect(screen.getByTestId('active-audits-trigger')).toBeInTheDocument();
    expect(screen.getByTestId('active-runs-trigger')).toBeInTheDocument();
    expect(screen.getByTestId('active-audits-trigger')).toHaveAccessibleName('Voir et arrêter 1 audit en cours');
    expect(screen.getByTestId('active-runs-trigger')).toHaveAccessibleName('Voir et arrêter 1 exécution en cours');
    expect(screen.getByTestId('active-audits-trigger')).toHaveAttribute('aria-haspopup', 'dialog');
    expect(screen.getByTestId('active-runs-trigger')).not.toHaveAttribute('aria-controls');
    expect(projectsTab?.querySelector('button')).toBeNull();
    expect(workflowsTab?.querySelector('button')).toBeNull();
    await act(async () => { fireEvent.click(screen.getByTestId('active-audits-trigger')); });
    expect(screen.getByRole('dialog', { name: 'Audits en cours' })).toBeInTheDocument();
    expect(screen.getByTestId('active-audits-trigger')).toHaveAttribute('aria-controls', 'active-audits-popover');
    await act(async () => { fireEvent.click(screen.getByTestId('active-runs-trigger')); });
    expect(screen.queryByRole('dialog', { name: 'Audits en cours' })).not.toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'Runs en cours' })).toBeInTheDocument();
    // A trigger pointerdown is not an outside click: its following click closes
    // this disclosure rather than reopening it from a stale outside handler.
    await act(async () => {
      fireEvent.pointerDown(screen.getByTestId('active-runs-trigger'));
      fireEvent.click(screen.getByTestId('active-runs-trigger'));
    });
    expect(screen.queryByRole('dialog', { name: 'Runs en cours' })).not.toBeInTheDocument();
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

  it('opens Discussions in a new tab from the running-agents badge on a Ctrl-click, and in place on a plain click', async () => {
    // The shared API mock has no `getRunning`: give this test one, and take it back.
    const api = discussionsApi as unknown as { getRunning?: () => Promise<string[]> };
    api.getRunning = vi.fn().mockResolvedValue(['disc-running']);
    const open = vi.spyOn(window, 'open').mockImplementation(() => null);
    try {
      await renderDashboard('/projects');
      const badge = await waitFor(() => {
        const found = document.querySelector<HTMLElement>('.dash-running-badge');
        if (!found) throw new Error('No running-agents badge yet');
        return found;
      });

      fireEvent.click(badge, { ctrlKey: true });
      expect(open).toHaveBeenCalledWith(`${window.location.origin}/discussions`, '_blank', 'noopener,noreferrer');
      expect(screen.queryByTestId('discussion-page')).toBeNull();

      await act(async () => { fireEvent.click(badge); });
      expect(await screen.findByTestId('discussion-page')).toBeInTheDocument();
      expect(window.location.pathname).toBe('/discussions');
      expect(open).toHaveBeenCalledTimes(1);
    } finally {
      open.mockRestore();
      delete api.getRunning;
    }
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

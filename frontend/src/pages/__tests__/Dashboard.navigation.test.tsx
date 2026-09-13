import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import type { DiscussionListItem } from '../../types/generated';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));

vi.mock('../DiscussionsPage', () => ({
  DiscussionsPage: ({
    initialActiveDiscussionId,
  }: {
    initialActiveDiscussionId?: string | null;
  }) => (
    <div data-testid="discussion-page">
      {initialActiveDiscussionId ?? 'discussion-list'}
    </div>
  ),
}));

vi.mock('../McpPage', () => ({ McpPage: () => <div data-testid="mcp-page" /> }));
vi.mock('../WorkflowsPage', () => ({ WorkflowsPage: () => <div data-testid="workflow-page" /> }));
vi.mock('../PlanningPage', () => ({ PlanningPage: () => <div data-testid="planning-page" /> }));
vi.mock('../SettingsPage', () => ({ SettingsPage: () => <div data-testid="settings-page" /> }));
vi.mock('../PagesPage', () => ({ PagesPage: () => <div data-testid="pages-page" /> }));

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { discussions as discussionsApi, pages as pagesApi, workflows as workflowsApi, projects as projectsApi } from '../../lib/api';
import { Dashboard } from '../Dashboard';

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

async function renderDashboard() {
  await act(async () => {
    render(
      <I18nProvider>
        <Dashboard onReset={vi.fn()} />
      </I18nProvider>,
    );
  });
}

beforeEach(() => {
  sessionStorage.clear();
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

describe('Dashboard reload/HMR navigation restoration', () => {
  it('always navigates Projects and Automation while activity disclosures remain separate buttons', async () => {
    vi.mocked(workflowsApi.list).mockResolvedValue([{
      id: 'wf-live', name: 'Live workflow', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0, enabled: true, pinned: false,
      last_run: { id: 'run-live', status: 'Running', started_at: '2026-01-01T00:00:00Z', finished_at: null, tokens_used: 0 },
      created_at: '2026-01-01T00:00:00Z',
    }]);
    vi.mocked(projectsApi.auditStatusAll).mockResolvedValue([{
      project_id: 'project-live', phase: 'auditing', step_index: 1, total_steps: 2,
      current_file: null, started_at: '2026-01-01T00:00:00Z', kind: 'full_audit',
    }]);
    await renderDashboard();
    const projectsTab = document.querySelector<HTMLButtonElement>('[data-tour-id="nav-projects"]');
    const workflowsTab = document.querySelector<HTMLButtonElement>('[data-tour-id="nav-workflows"]');
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
  it('reveals Pages only after the first Page has activated the capability', async () => {
    await renderDashboard();
    expect(screen.queryByRole('button', { name: 'Pages' })).not.toBeInTheDocument();
    cleanup();

    vi.mocked(pagesApi.capability).mockResolvedValue({
      activated: true,
      activated_at: '2026-08-13T10:00:00Z',
    });
    await renderDashboard();

    const button = await screen.findByRole('button', { name: 'Pages' });
    const navOrder = Array.from(
      document.querySelectorAll<HTMLButtonElement>('.dash-nav-tabs [data-tour-id^="nav-"]'),
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

  it('reveals Pages immediately when a workflow import activates the capability', async () => {
    vi.mocked(pagesApi.capability)
      .mockResolvedValueOnce({ activated: false, activated_at: null })
      .mockResolvedValueOnce({ activated: true, activated_at: '2026-08-26T16:00:00Z' });
    await renderDashboard();
    expect(screen.queryByRole('button', { name: 'Pages' })).not.toBeInTheDocument();

    await act(async () => {
      window.dispatchEvent(new Event('kronn:pages-activated'));
    });

    const pagesButton = await screen.findByRole('button', { name: 'Pages' });
    expect(pagesApi.capability).toHaveBeenCalledTimes(2);
    pagesButton.click();
    expect(await screen.findByTestId('pages-page')).toBeInTheDocument();
  });

  it('restores the Discussions page and its existing active discussion', async () => {
    sessionStorage.setItem('kronn:navigation:page', 'discussions');
    sessionStorage.setItem('kronn:navigation:discussion', 'disc-42');
    vi.mocked(discussionsApi.list).mockResolvedValue([makeDiscussion('disc-42')]);

    await renderDashboard();

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
    window.location.hash = '#discussion-disc-deep';
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await renderDashboard();

    expect(await screen.findByTestId('discussion-page')).toHaveTextContent('disc-deep');
    window.location.hash = '';
  });

  it('drops a stale discussion id and keeps the safe list view', async () => {
    sessionStorage.setItem('kronn:navigation:page', 'discussions');
    sessionStorage.setItem('kronn:navigation:discussion', 'deleted-disc');
    vi.mocked(discussionsApi.list).mockResolvedValue([]);

    await renderDashboard();

    expect(await screen.findByTestId('discussion-page')).toHaveTextContent('discussion-list');
    await waitFor(() => {
      expect(sessionStorage.getItem('kronn:navigation:discussion')).toBeNull();
    });
  });
});

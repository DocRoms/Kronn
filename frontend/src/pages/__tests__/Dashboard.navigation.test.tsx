import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import type { Discussion } from '../../types/generated';

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

vi.mock('../../lib/api', () => ({
  projects: {
    list: vi.fn().mockResolvedValue([]),
    auditStatusAll: vi.fn().mockResolvedValue([]),
    auditHistory: vi.fn().mockResolvedValue([]),
  },
  mcps: {
    registry: vi.fn().mockResolvedValue([]),
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], customized_contexts: [] }),
  },
  agents: {
    detect: vi.fn().mockResolvedValue([]),
  },
  discussions: {
    list: vi.fn().mockResolvedValue([]),
    runAgent: vi.fn(),
    sendMessageStream: vi.fn(),
  },
  workflows: {
    list: vi.fn().mockResolvedValue([]),
  },
  pages: { capability: vi.fn().mockResolvedValue({ activated: false, activated_at: null }) },
  config: {
    getLanguage: vi.fn().mockResolvedValue('fr'),
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
    getSttModel: vi.fn().mockResolvedValue(null),
    getTtsVoices: vi.fn().mockResolvedValue({}),
    getAgentAccess: vi.fn().mockResolvedValue(null),
  },
  skills: {
    list: vi.fn().mockResolvedValue([]),
  },
}));

import { discussions as discussionsApi, pages as pagesApi } from '../../lib/api';
import { Dashboard } from '../Dashboard';

const makeDiscussion = (id: string): Discussion => ({
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
  summary_strategy: 'Auto',
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
});

afterEach(() => {
  cleanup();
  sessionStorage.clear();
  vi.clearAllMocks();
});

describe('Dashboard reload/HMR navigation restoration', () => {
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

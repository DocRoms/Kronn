import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import type { Discussion } from '../../types/generated';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false })),
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

vi.mock('../../lib/api', () => ({
  projects: {
    list: vi.fn().mockResolvedValue([]),
    auditStatusAll: vi.fn().mockResolvedValue([]),
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

import { discussions as discussionsApi } from '../../lib/api';
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

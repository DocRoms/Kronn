// KT-987 — drift hashes every mapped source of a project and only the
// Projects page shows it. It was requested for every audited project each
// time the project list refreshed, on every page.
import { act, cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import type { Project } from '../../types/generated';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));
vi.mock('../DiscussionsPage', () => ({ DiscussionsPage: () => <div data-testid="discussion-page" /> }));
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

const project = (id: string, audit_status: Project['audit_status']): Project => ({
  id,
  name: id,
  path: `/repos/${id}`,
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status,
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
});

async function renderDashboard() {
  await act(async () => {
    render(<I18nProvider><Dashboard onReset={vi.fn()} /></I18nProvider>);
  });
}

async function openPage(tourId: string) {
  await act(async () => {
    fireEvent.click(document.querySelector<HTMLElement>(`[data-tour-id="${tourId}"]`)!);
  });
}

beforeEach(() => {
  sessionStorage.clear();
  window.location.hash = '';
  vi.mocked(discussionsApi.list).mockResolvedValue([]);
  vi.mocked(workflowsApi.list).mockResolvedValue([]);
  vi.mocked(projectsApi.auditStatusAll).mockResolvedValue([]);
  vi.mocked(pagesApi.capability).mockResolvedValue({ activated: false, activated_at: null });
  vi.mocked(projectsApi.list).mockResolvedValue([project('audited', 'Audited'), project('bare', 'NoTemplate')]);
  vi.mocked(projectsApi.checkDrift).mockResolvedValue({ audit_date: null, stale_sections: [], fresh_sections: [], total_sections: 0 });
});

afterEach(() => {
  cleanup();
  sessionStorage.clear();
  vi.clearAllMocks();
});

describe('Dashboard — drift requests', () => {
  it('asks nothing outside the Projects page', async () => {
    sessionStorage.setItem('kronn:navigation:page', 'discussions');
    await renderDashboard();
    expect(projectsApi.checkDrift).not.toHaveBeenCalled();
  });

  it('asks once per audited project, not again when the page is revisited', async () => {
    sessionStorage.setItem('kronn:navigation:page', 'projects');
    await renderDashboard();
    expect(vi.mocked(projectsApi.checkDrift).mock.calls).toEqual([['audited']]);

    await openPage('nav-discussions');
    await openPage('nav-projects');

    expect(projectsApi.checkDrift).toHaveBeenCalledTimes(1);
  });
});

// KT-1148 — Automation and Planning are memoized; that only helps if the
// Dashboard hands them the same props when one of its polls re-renders it.
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';

const pageRenders = vi.hoisted(() => ({
  workflows: [] as Record<string, unknown>[],
  planning: [] as Record<string, unknown>[],
}));

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));
vi.mock('../DiscussionsPage', () => ({ DiscussionsPage: () => <div data-testid="discussion-page" /> }));
vi.mock('../McpPage', () => ({ McpPage: () => <div data-testid="mcp-page" /> }));
vi.mock('../SettingsPage', () => ({ SettingsPage: () => <div data-testid="settings-page" /> }));
vi.mock('../PagesPage', () => ({ PagesPage: () => <div data-testid="pages-page" /> }));
vi.mock('../WorkflowsPage', () => ({
  WorkflowsPage: (props: Record<string, unknown>) => {
    pageRenders.workflows.push(props);
    return <div data-testid="workflow-page" />;
  },
}));
vi.mock('../PlanningPage', () => ({
  PlanningPage: (props: Record<string, unknown>) => {
    pageRenders.planning.push(props);
    return <div data-testid="planning-page" />;
  },
}));
vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { discussions as discussionsApi, pages as pagesApi, workflows as workflowsApi, projects as projectsApi } from '../../lib/api';
import { Dashboard } from '../Dashboard';

const sameDiscussions: never[] = [];

beforeEach(() => {
  sessionStorage.clear();
  window.location.hash = '';
  pageRenders.workflows.length = 0;
  pageRenders.planning.length = 0;
  vi.mocked(discussionsApi.list).mockResolvedValue(sameDiscussions);
  // A fresh array on every poll, as the real endpoint returns.
  vi.mocked(workflowsApi.list).mockImplementation(async () => []);
  vi.mocked(projectsApi.auditStatusAll).mockResolvedValue([]);
  vi.mocked(pagesApi.capability).mockResolvedValue({ activated: false, activated_at: null });
});

afterEach(() => {
  cleanup();
  sessionStorage.clear();
  vi.clearAllMocks();
});

async function openTab(id: 'workflows' | 'planning', testId: string) {
  await act(async () => {
    render(<I18nProvider><Dashboard onReset={vi.fn()} /></I18nProvider>);
  });
  await act(async () => {
    document.querySelector<HTMLButtonElement>(`[data-tour-id="nav-${id}"]`)?.click();
  });
  expect(await screen.findByTestId(testId)).toBeInTheDocument();
}

async function pollAgain() {
  // Returning to the tab refetches the workflow list right away.
  await act(async () => {
    document.dispatchEvent(new Event('visibilitychange'));
    await new Promise(resolve => setTimeout(resolve, 0));
  });
}

function expectSameProps(renders: Record<string, unknown>[]) {
  const before = renders.length;
  return () => {
    expect(renders.length, 'the poll must re-render the Dashboard').toBeGreaterThan(before);
    const first = renders[before - 1];
    const last = renders[renders.length - 1];
    expect(Object.keys(last).sort()).toEqual(Object.keys(first).sort());
    for (const key of Object.keys(first)) {
      expect(Object.is(last[key], first[key]), `prop ${key} changed identity`).toBe(true);
    }
  };
}

describe('Dashboard page props across a poll', () => {
  beforeEach(() => {
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
  });

  it('hands Automation the same props', async () => {
    await openTab('workflows', 'workflow-page');
    const check = expectSameProps(pageRenders.workflows);
    await pollAgain();
    check();
  });

  it('hands Planning the same props', async () => {
    await openTab('planning', 'planning-page');
    const check = expectSameProps(pageRenders.planning);
    await pollAgain();
    check();
  });
});

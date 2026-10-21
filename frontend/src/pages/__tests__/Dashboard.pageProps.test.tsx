// KT-1148 — Automation and Planning are memoized; that only helps if their
// route components hand them the same props when a Dashboard poll re-renders
// them. Rendered through the real router and route table, as in production.
import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { withDashboardRoutes } from '../../test/routerWrapper';
import { tourAlreadyTaken } from '../../test/tour';

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
  localStorage.clear();
  tourAlreadyTaken();
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
  localStorage.clear();
  vi.clearAllMocks();
});

async function openPage(path: '/workflows' | '/planning', testId: string) {
  await act(async () => {
    render(<I18nProvider>{withDashboardRoutes(<Dashboard onReset={vi.fn()} />, path)}</I18nProvider>);
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

describe('Route page props across a Dashboard poll', () => {
  beforeEach(() => {
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
  });

  it('hands Automation the same props', async () => {
    await openPage('/workflows', 'workflow-page');
    const check = expectSameProps(pageRenders.workflows);
    await pollAgain();
    check();
  });

  it('hands Planning the same props', async () => {
    await openPage('/planning', 'planning-page');
    const check = expectSameProps(pageRenders.planning);
    await pollAgain();
    check();
  });
});

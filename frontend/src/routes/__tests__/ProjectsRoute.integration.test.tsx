// The Projects page behind the real router and the real dashboard shell,
// with only the API boundary simulated: the file open in a project's code
// view and its address, through a click in the tree, Back, Forward and a
// reload.
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { withDashboardRoutes } from '../../test/routerWrapper';
import { tourAlreadyTaken } from '../../test/tour';
import type { Project } from '../../types/generated';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));
vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { pages as pagesApi, projects as projectsApi } from '../../lib/api';
import { Dashboard } from '../../pages/Dashboard';

const project: Project = {
  id: 'p1', name: 'Demo', path: '/repos/demo', repo_url: null, token_override: null,
  ai_config: { detected: false, configs: [] }, audit_status: 'NoTemplate',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  write_access: { status: 'Writable' }, mcp_sync_report: null,
  created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
};

async function renderDashboard(initialPath: string) {
  await act(async () => {
    render(<I18nProvider>{withDashboardRoutes(<Dashboard onReset={vi.fn()} />, initialPath)}</I18nProvider>);
  });
}

const address = () => `${window.location.pathname}${window.location.search}`;
const selectedFile = () => document.querySelector<HTMLElement>('.source-tree-file[data-selected="true"]')?.textContent ?? null;

beforeEach(() => {
  localStorage.clear();
  tourAlreadyTaken();
  vi.mocked(pagesApi.capability).mockResolvedValue({ activated: false, activated_at: null });
  vi.mocked(projectsApi.list).mockResolvedValue([project]);
  vi.mocked(projectsApi.listSourceFiles).mockResolvedValue({
    entries: [
      { path: 'README.md', name: 'README.md', is_dir: false },
      { path: 'NOTES.md', name: 'NOTES.md', is_dir: false },
    ],
    truncated: false,
  });
  vi.mocked(projectsApi.readSourceFile).mockImplementation(async (_id, path) => ({ path, content: `contents of ${path}` }));
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
});

describe('Projects route — the file open in the code view', () => {
  it('writes the file the reader opens into the address; Back, Forward and a reload follow it', async () => {
    await renderDashboard('/projects/p1/code?file=README.md');
    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();
    expect(selectedFile()).toBe('README.md');
    const depth = window.history.length;

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'NOTES.md' })); });
    expect(await screen.findByText('contents of NOTES.md')).toBeInTheDocument();
    expect(address()).toBe('/projects/p1/code?file=NOTES.md');
    expect(window.history.length).toBe(depth + 1);

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(address()).toBe('/projects/p1/code?file=README.md'));
    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();
    expect(selectedFile()).toBe('README.md');

    await act(async () => { window.history.forward(); });
    await waitFor(() => expect(address()).toBe('/projects/p1/code?file=NOTES.md'));
    expect(await screen.findByText('contents of NOTES.md')).toBeInTheDocument();
    expect(selectedFile()).toBe('NOTES.md');

    // A reload: the copied address reopens the file that was being read.
    cleanup();
    await renderDashboard('/projects/p1/code?file=NOTES.md');
    expect(await screen.findByText('contents of NOTES.md')).toBeInTheDocument();
    expect(selectedFile()).toBe('NOTES.md');
  });

  it('keeps the tree it loaded when the address moves to another file', async () => {
    await renderDashboard('/projects/p1/code?file=README.md');
    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();
    const treeRequests = vi.mocked(projectsApi.listSourceFiles).mock.calls.length;

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'NOTES.md' })); });
    expect(await screen.findByText('contents of NOTES.md')).toBeInTheDocument();
    await act(async () => { window.history.back(); });
    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();

    expect(vi.mocked(projectsApi.listSourceFiles).mock.calls).toHaveLength(treeRequests);
  });
});

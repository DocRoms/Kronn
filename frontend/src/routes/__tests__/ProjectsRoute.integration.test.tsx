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

  it('names the file it opens on its own without a step of history; Back to it reopens it', async () => {
    await renderDashboard('/projects/p1/code');
    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();
    // The default file is written into the address in place: the bare
    // address is the same page, not a step Back has to undo.
    await waitFor(() => expect(address()).toBe('/projects/p1/code?file=README.md'));
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

    // A reload of the bare address opens the default file again.
    cleanup();
    await renderDashboard('/projects/p1/code');
    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();
    expect(selectedFile()).toBe('README.md');
  });

  it('opens the default file again when the address stops naming one, in the tree it has', async () => {
    await renderDashboard('/projects/p1/code?file=NOTES.md');
    expect(await screen.findByText('contents of NOTES.md')).toBeInTheDocument();
    const treeRequests = vi.mocked(projectsApi.listSourceFiles).mock.calls.length;

    // An address of the code view that names no file, reached without leaving
    // the view: a link, an entry written before the default was named.
    await act(async () => {
      window.history.pushState(null, '', '/projects/p1/code');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();
    expect(selectedFile()).toBe('README.md');
    await waitFor(() => expect(address()).toBe('/projects/p1/code?file=README.md'));
    expect(vi.mocked(projectsApi.listSourceFiles).mock.calls).toHaveLength(treeRequests);
  });

  it('applies the address of the moment to a tree that arrives late: no file named, the default file', async () => {
    let deliverTree!: () => void;
    vi.mocked(projectsApi.listSourceFiles).mockImplementationOnce(() => new Promise(resolve => {
      deliverTree = () => resolve({
        entries: [
          { path: 'README.md', name: 'README.md', is_dir: false },
          { path: 'NOTES.md', name: 'NOTES.md', is_dir: false },
        ],
        truncated: false,
      });
    }));
    await renderDashboard('/projects/p1/code?file=NOTES.md');

    // The Code tab, while the tree is on its way: the address names no file.
    await act(async () => {
      fireEvent.click(screen.getAllByRole('button').find(button => button.closest('nav.project-detail-tabs') && button.textContent?.trim() === 'Code')!);
    });
    await waitFor(() => expect(address()).toBe('/projects/p1/code'));
    const depth = window.history.length;

    await act(async () => { deliverTree(); });

    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();
    expect(selectedFile()).toBe('README.md');
    // Named in place: the canonical address is not a step of history.
    await waitFor(() => expect(address()).toBe('/projects/p1/code?file=README.md'));
    expect(window.history.length).toBe(depth);

    // A reload of that address shows the same file.
    cleanup();
    await renderDashboard('/projects/p1/code?file=README.md');
    expect(await screen.findByText('contents of README.md')).toBeInTheDocument();
    expect(selectedFile()).toBe('README.md');
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

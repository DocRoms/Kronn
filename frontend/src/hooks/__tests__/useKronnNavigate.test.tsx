import { act, cleanup, render } from '@testing-library/react';
import { createMemoryRouter, RouterProvider, useLocation } from 'react-router';
import { afterEach, describe, expect, it } from 'vitest';
import { PAGE_PATHS, type DashboardPage } from '../../lib/routes';
import { useKronnNavigate, type KronnNavigate } from '../useKronnNavigate';

const seen: KronnNavigate[] = [];

// Reads the location so that it re-renders on every navigation.
function Probe() {
  useLocation();
  seen.push(useKronnNavigate());
  return null;
}

function mount(initialEntries: string[]) {
  const router = createMemoryRouter([{ path: '*', element: <Probe /> }], { initialEntries });
  render(<RouterProvider router={router} useTransitions={false} />);
  return router;
}

const latest = () => seen[seen.length - 1];

afterEach(() => {
  cleanup();
  seen.length = 0;
});

describe('useKronnNavigate', () => {
  it.each(Object.entries(PAGE_PATHS) as [DashboardPage, string][])('goes to %s at %s', async (page, path) => {
    const router = mount(['/somewhere']);

    await act(async () => latest().toPage(page));

    expect(router.state.location.pathname).toBe(path);
  });

  it('opens a project at its own address', async () => {
    const router = mount(['/discussions']);

    await act(async () => latest().toProject('proj/1'));
    expect(router.state.location.pathname).toBe('/projects/proj%2F1');
    expect(router.state.historyAction).toBe('PUSH');

    await act(async () => latest().toProject('proj-2', { replace: true }));
    expect(router.state.location.pathname).toBe('/projects/proj-2');
    expect(router.state.historyAction).toBe('REPLACE');
  });

  it('opens a discussion at its own address, carrying what to do on arrival', async () => {
    const router = mount(['/projects']);

    await act(async () => latest().toDiscussion('disc/1'));
    expect(router.state.location.pathname).toBe('/discussions/disc%2F1');
    expect(router.state.location.state).toBeNull();

    await act(async () => latest().toDiscussion('disc-2', { autoRun: true }));
    expect(router.state.location.state).toEqual({ autoRun: true });

    await act(async () => latest().toDiscussion('disc-3', { focusBatch: { id: 'batch-1', mode: 'compare' }, replace: true }));
    expect(router.state.location.pathname).toBe('/discussions/disc-3');
    expect(router.state.location.state).toEqual({ focusBatch: { id: 'batch-1', mode: 'compare' } });
    expect(router.state.historyAction).toBe('REPLACE');
  });

  it('opens a planning task at its own address', async () => {
    const router = mount(['/projects']);

    await act(async () => latest().toPlanningTask('task/4'));
    expect(router.state.location.pathname).toBe('/planning/task%2F4');
    expect(router.state.historyAction).toBe('PUSH');

    await act(async () => latest().toPlanningTask('task-5', { replace: true }));
    expect(router.state.location.pathname).toBe('/planning/task-5');
    expect(router.state.historyAction).toBe('REPLACE');
  });

  it('opens a workflow, a run, a tab and resource of Automation at their addresses', async () => {
    const router = mount(['/projects']);

    await act(async () => latest().toWorkflow('wf-1'));
    expect(router.state.location.pathname).toBe('/workflows/wf-1');

    await act(async () => latest().toWorkflow('wf-1', 'run-2'));
    expect(router.state.location.pathname).toBe('/workflows/wf-1/runs/run-2');

    await act(async () => latest().toAutomation({ tab: 'quickApis', resourceId: 'qa-1' }, { replace: true }));
    expect(router.state.location.pathname).toBe('/workflows/qa/qa-1');
    expect(router.state.historyAction).toBe('REPLACE');
  });

  it('opens a plugin config at its own address', async () => {
    const router = mount(['/projects']);

    await act(async () => latest().toPlugin('cfg/9'));
    expect(router.state.location.pathname).toBe('/plugins/cfg%2F9');
    expect(router.state.historyAction).toBe('PUSH');

    await act(async () => latest().toPlugin('cfg-1', { replace: true }));
    expect(router.state.historyAction).toBe('REPLACE');
  });

  it('opens a Page at its own address on the Artifacts page', async () => {
    const router = mount(['/projects']);

    await act(async () => latest().toLivePage('page/7'));
    expect(router.state.location.pathname).toBe('/pages/page%2F7');

    await act(async () => latest().toLivePage('page-8', { replace: true }));
    expect(router.state.historyAction).toBe('REPLACE');
  });

  it('opens Automation as it was left, carrying a wizard preset when there is one', async () => {
    const router = mount(['/projects']);

    await act(async () => latest().toWorkflows());
    expect(router.state.location.pathname).toBe('/workflows');
    expect(router.state.location.state).toBeNull();

    await act(async () => latest().toWorkflows({ preset: { presetId: 'ticket-to-pr', projectId: 'proj-1' } }));
    expect(router.state.location.state).toEqual({ preset: { presetId: 'ticket-to-pr', projectId: 'proj-1' } });
  });

  it('opens the Discussions page as it was left, with or without an intent', async () => {
    const router = mount(['/projects']);

    await act(async () => latest().toDiscussions());
    expect(router.state.location.pathname).toBe('/discussions');
    expect(router.state.location.state).toBeNull();

    await act(async () => latest().toDiscussions({ focusBatch: { id: 'batch-1', mode: 'batch' } }));
    expect(router.state.location.state).toEqual({ focusBatch: { id: 'batch-1', mode: 'batch' } });
  });

  it('adds a history entry by default, so Back returns to where the user was', async () => {
    const router = mount(['/projects']);

    await act(async () => latest().toPage('planning'));
    expect(router.state.historyAction).toBe('PUSH');

    await act(async () => { await router.navigate(-1); });
    expect(router.state.location.pathname).toBe('/projects');
  });

  it('replaces the current entry on request, so a redirect leaves no trace', async () => {
    const router = mount(['/projects', '/planning#project-proj-1']);

    await act(async () => latest().toPage('workflows', { replace: true }));
    expect(router.state.historyAction).toBe('REPLACE');
    expect(router.state.location.pathname).toBe('/workflows');

    await act(async () => { await router.navigate(-1); });
    expect(router.state.location.pathname).toBe('/projects');
  });

  it('lands on the bare page address: neither hash nor query survives', async () => {
    const router = mount(['/planning?tab=x#project-proj-1']);

    await act(async () => latest().toPage('projects'));

    expect(router.state.location.search).toBe('');
    expect(router.state.location.hash).toBe('');
  });

  it('keeps its identity across renders and navigations', async () => {
    mount(['/projects']);
    const first = latest();

    await act(async () => first.toPage('planning'));

    expect(seen.length).toBeGreaterThan(1);
    expect(latest()).toBe(first);
    expect(latest().toPage).toBe(first.toPage);
  });
});

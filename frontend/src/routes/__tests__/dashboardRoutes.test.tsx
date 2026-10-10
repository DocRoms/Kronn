import { useEffect, useState, type ComponentProps } from 'react';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { Outlet } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DashboardOutletContext } from '../../lib/dashboardContext';
import { I18nProvider } from '../../lib/I18nContext';
import type { McpOverview } from '../../types/generated';
import type { ProjectList } from '../../components/ProjectList';
import type { DiscussionsPage } from '../../pages/DiscussionsPage';
import type { WorkflowsPage } from '../../pages/WorkflowsPage';
import type { PagesPage } from '../../pages/PagesPage';
import type { PlanningPage } from '../../pages/PlanningPage';
import type { SettingsPage } from '../../pages/SettingsPage';
import type { McpPage } from '../../pages/McpPage';
import { withDashboardRoutes } from '../../test/routerWrapper';
import { navigateAppTab } from '../../lib/live-page-navigation';

// Each page is replaced by a marker that records the props its route gave it,
// so a test can read what the page received and fire what it would call.
const page = vi.hoisted(() => ({
  props: {} as Record<string, unknown>,
  mounts: {} as Record<string, number>,
  failing: null as string | null,
}));

function marker(name: string) {
  return (props: unknown) => {
    if (page.failing === name) throw new Error(`${name} exploded`);
    page.props[name] = props;
    useEffect(() => { page.mounts[name] = (page.mounts[name] ?? 0) + 1; }, []);
    return <div data-testid={`${name}-page`} />;
  };
}

vi.mock('../../components/ProjectList', () => ({ ProjectList: marker('projects') }));
vi.mock('../../pages/DiscussionsPage', () => ({ DiscussionsPage: marker('discussions') }));
vi.mock('../../pages/PlanningPage', () => ({ PlanningPage: marker('planning') }));
vi.mock('../../pages/WorkflowsPage', () => ({ WorkflowsPage: marker('workflows') }));
vi.mock('../../pages/PagesPage', () => ({ PagesPage: marker('pages') }));
vi.mock('../../pages/McpPage', () => ({ McpPage: marker('mcps') }));
vi.mock('../../pages/SettingsPage', () => ({ SettingsPage: marker('settings') }));

const received = {
  projects: () => page.props.projects as ComponentProps<typeof ProjectList>,
  discussions: () => page.props.discussions as ComponentProps<typeof DiscussionsPage>,
  planning: () => page.props.planning as ComponentProps<typeof PlanningPage>,
  workflows: () => page.props.workflows as ComponentProps<typeof WorkflowsPage>,
  pages: () => page.props.pages as ComponentProps<typeof PagesPage>,
  mcps: () => page.props.mcps as ComponentProps<typeof McpPage>,
  settings: () => page.props.settings as ComponentProps<typeof SettingsPage>,
};

const emptyOverview: McpOverview = {
  servers: [], configs: [], customized_contexts: [], incompatibilities: [], incomplete_configs: [],
};

function makeContext(overrides: Partial<DashboardOutletContext> = {}): DashboardOutletContext {
  return {
    projects: [],
    projectsLoading: false,
    projectsLoaded: true,
    agents: [],
    allDiscussions: [],
    discussionsByProject: {},
    allSkills: [],
    workflowList: [],
    activeAudits: [],
    driftByProject: {},
    configLanguage: 'fr',
    agentAccess: null,
    mcpOverview: emptyOverview,
    mcpOverviewLoaded: true,
    mcpRegistry: [],
    pagesCapability: { activated: true, activated_at: '2026-08-13T10:00:00Z' },
    refetchProjects: vi.fn(),
    refetchDiscussions: vi.fn(),
    refetchSkills: vi.fn(),
    refetchAgents: vi.fn(),
    refetchAgentAccess: vi.fn(),
    refetchLanguage: vi.fn(),
    refetchMcps: vi.fn(),
    refetchDrift: vi.fn(),
    toast: vi.fn(),
    onReset: vi.fn(),
    openAddProject: vi.fn(),
    markBatchSending: vi.fn(),
    sendingMap: {},
    setSendingMap: vi.fn(),
    queuedMap: {},
    setQueuedMap: vi.fn(),
    sendingStartMap: {},
    setSendingStartMap: vi.fn(),
    streamingMap: {},
    setStreamingMap: vi.fn(),
    noteStreamTick: vi.fn(),
    abortControllers: { current: {} },
    cleanupStream: vi.fn(),
    lastSeenMsgCount: {},
    markDiscussionSeen: vi.fn(),
    markAllDiscussionsSeen: vi.fn(),
    discPrefill: null,
    setDiscPrefill: vi.fn(),
    restorableDiscussionId: null,
    setActiveDiscussionId: vi.fn(),
    ...overrides,
  };
}

// Stands in for the dashboard shell. `shell.update` re-renders it with a new
// context, as a poll or a stream chunk does to the real one.
const shell: { update: (context: DashboardOutletContext) => void } = { update: () => {} };

function Shell({ context: initialContext }: { context: DashboardOutletContext }) {
  const [context, setContext] = useState(initialContext);
  useEffect(() => { shell.update = setContext; }, []);
  return <Outlet context={context} />;
}

async function open(path: string, context = makeContext()) {
  await act(async () => {
    render(<I18nProvider>{withDashboardRoutes(<Shell context={context} />, path)}</I18nProvider>);
  });
  return context;
}

beforeEach(() => {
  page.props = {};
  page.mounts = {};
  page.failing = null;
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('dashboard route table', () => {
  it.each([
    ['/projects', 'projects'],
    ['/discussions', 'discussions'],
    ['/planning', 'planning'],
    ['/workflows', 'workflows'],
    ['/pages', 'pages'],
    ['/plugins', 'mcps'],
    ['/config', 'settings'],
  ])('renders %s as the %s page', async (path, name) => {
    await open(path);

    expect(screen.getByTestId(`${name}-page`)).toBeInTheDocument();
    expect(window.location.pathname).toBe(path);
  });

  it('keeps the hash and the query of a bare address it redirects', async () => {
    await open('/?from=cli#page/page-7');

    expect(window.location.pathname).toBe('/projects');
    expect(window.location.search).toBe('?from=cli');
    expect(window.location.hash).toBe('#page/page-7');
  });

  it('redirects an unknown address to the default page', async () => {
    await open('/does/not/exist');

    expect(screen.getByTestId('projects-page')).toBeInTheDocument();
    expect(window.location.pathname).toBe('/projects');
  });

  it('contains a crashing page in its own zone and recovers on the next page', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => {});
    page.failing = 'planning';
    await open('/planning');
    expect(screen.getByText(/Planning — /)).toBeInTheDocument();
    expect(screen.getByText('planning exploded')).toBeInTheDocument();

    // The next page must not inherit the failed zone.
    await act(async () => {
      window.history.pushState(null, '', '/config');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });

    expect(await screen.findByTestId('settings-page')).toBeInTheDocument();
    expect(screen.queryByText('planning exploded')).not.toBeInTheDocument();
  });
});

describe('selection callbacks', () => {
  // Every owned page lists its callback in an effect's dependencies. A new
  // identity after Back would re-run that effect, restate the previous
  // selection and overwrite the address Back just restored.
  it.each([
    ['projects', '/projects/a', '/projects/b', 'onSetExpandedId'],
    ['discussions', '/discussions/a', '/discussions/b', 'onActiveDiscussionChange'],
    ['planning', '/planning/a', '/planning/b', 'onSelectedTaskChange'],
    ['workflows', '/workflows/qp/a', '/workflows/wf-1/runs/r', 'onSelectionChange'],
    ['mcps', '/plugins/a', '/plugins/b', 'onSelectedConfigChange'],
    ['pages', '/pages/a', '/pages/b', 'onSelectedPageChange'],
  ] as const)('%s keeps its callback identity when the address changes', async (name, first, second, callback) => {
    await open(first);
    const before = (page.props[name] as Record<string, unknown>)[callback];
    expect(before).toBeTypeOf('function');

    await act(async () => {
      window.history.pushState(null, '', second);
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    await waitFor(() => expect(window.location.pathname).toBe(second));

    expect((page.props[name] as Record<string, unknown>)[callback]).toBe(before);
  });

  it.each([
    ['discussions', '/discussions/a', '/discussions/b'],
    ['workflows', '/workflows/qp/a', '/workflows/qp/b'],
  ] as const)('%s receives a new address token on every navigation, Back to the first entry included', async (name, first, second) => {
    await open(first);
    const token = () => (page.props[name] as { addressToken?: object }).addressToken;
    const atFirst = token();
    expect(atFirst).toBeTypeOf('object');

    await act(async () => {
      window.history.pushState(null, '', second);
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    await waitFor(() => expect(window.location.pathname).toBe(second));
    const atSecond = token();
    expect(atSecond).not.toBe(atFirst);

    // Back lands on the first entry: same address, same key, new token.
    await act(async () => { window.history.back(); });
    await waitFor(() => expect(window.location.pathname).toBe(first));
    expect(token()).not.toBe(atFirst);
    expect(token()).not.toBe(atSecond);
  });

  it('a page restating its selection after Back is heard against the restored address', async () => {
    await open('/discussions/a');
    act(() => received.discussions().onActiveDiscussionChange('b'));
    expect(window.location.pathname).toBe('/discussions/b');

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(window.location.pathname).toBe('/discussions/a'));
    const depth = window.history.length;

    // The page follows the address and confirms it: nothing moves.
    act(() => received.discussions().onActiveDiscussionChange('a'));
    expect(window.location.pathname).toBe('/discussions/a');
    expect(window.history.length).toBe(depth);
  });
});

describe('Artifacts route', () => {
  it('renders nothing while the capability is unknown, and stays on its address', async () => {
    await open('/pages', makeContext({ pagesCapability: null }));

    expect(screen.queryByTestId('pages-page')).not.toBeInTheDocument();
    expect(window.location.pathname).toBe('/pages');
  });

  it('leaves for the default page when the capability is not activated', async () => {
    await open('/pages', makeContext({ pagesCapability: { activated: false, activated_at: null } }));

    await waitFor(() => expect(window.location.pathname).toBe('/projects'));
    expect(screen.queryByTestId('pages-page')).not.toBeInTheDocument();
  });

  it('reads the open Page from its address, decoded', async () => {
    await open('/pages/page%2F7');

    expect(received.pages().selectedPageId).toBe('page/7');
    expect(received.pages().initialSelectedPageId).toBeUndefined();
  });

  it('moves the address when the reader picks a Page, a step Back undoes', async () => {
    await open('/pages/page-1');
    const depth = window.history.length;

    act(() => received.pages().onSelectedPageChange?.('page-2'));
    expect(window.location.pathname).toBe('/pages/page-2');
    expect(window.history.length).toBe(depth + 1);
    expect(received.pages().selectedPageId).toBe('page-2');

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(received.pages().selectedPageId).toBe('page-1'));
  });

  it('lets the Page the library opens on its own replace the bare address', async () => {
    await open('/pages');
    const depth = window.history.length;

    act(() => received.pages().onSelectedPageChange?.(null));
    expect(window.location.pathname).toBe('/pages');
    act(() => received.pages().onSelectedPageChange?.('page-first'));

    expect(window.location.pathname).toBe('/pages/page-first');
    expect(window.history.length).toBe(depth);
  });

  it('jumps to a workflow run or a discussion', async () => {
    await open('/pages');

    await act(async () => received.pages().onNavigateWorkflow?.('wf-1', 'run-2'));
    expect(window.location.pathname).toBe('/workflows/wf-1/runs/run-2');
    expect(received.workflows().selection).toEqual({ tab: 'workflows', resourceId: 'wf-1', runId: 'run-2' });

    await act(async () => received.pages().onNavigateWorkflow?.('wf-3'));
    expect(window.location.pathname).toBe('/workflows/wf-3');
    expect(received.workflows().selection).toEqual({ tab: 'workflows', resourceId: 'wf-3', runId: null });
  });
});

describe('Projects route', () => {
  it('maps the fleet and the shell services to the list', async () => {
    const ctx = await open('/projects', makeContext({ projectsLoading: true, projectsLoaded: false }));
    const props = received.projects();

    expect(props.expandedId).toBeNull();
    expect(props.loading).toBe(true);
    expect(props.favoritesReady).toBe(false);
    expect(props.modelTiers).toBeNull();
    props.onAddProject?.();
    expect(ctx.openAddProject).toHaveBeenCalledOnce();
    props.onRefetchDrift('proj-1');
    expect(ctx.refetchDrift).toHaveBeenCalledWith('proj-1');
  });

  it('reads the open project from its address', async () => {
    await open('/projects/proj-1');

    expect(received.projects().expandedId).toBe('proj-1');
    expect(window.location.pathname).toBe('/projects/proj-1');
  });

  it('decodes the project id the address carries', async () => {
    await open('/projects/a%2Fb%20c');

    expect(received.projects().expandedId).toBe('a/b c');
  });

  it('moves the address when the reader picks a project, a step Back undoes', async () => {
    await open('/projects/proj-1');
    const depth = window.history.length;

    await act(async () => received.projects().onSetExpandedId('proj-2'));
    expect(window.history.length).toBe(depth + 1);
    expect(received.projects().expandedId).toBe('proj-2');
    expect(window.location.pathname).toBe('/projects/proj-2');

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(received.projects().expandedId).toBe('proj-1'));
  });

  it('lets the first project the page opens on its own replace the bare address', async () => {
    await open('/projects');
    const depth = window.history.length;

    await act(async () => received.projects().onSetExpandedId('proj-first'));

    // No new entry: Back leaves the Projects page instead of bouncing to `/projects`.
    expect(window.location.pathname).toBe('/projects/proj-first');
    expect(window.history.length).toBe(depth);
  });

  it('hands the project the view its address names, and follows the view the reader picks', async () => {
    await open('/projects/proj-1/code?file=src%2Fmain.ts&line=7');
    expect(received.projects().expandedId).toBe('proj-1');
    expect(received.projects().projectLocation).toEqual({ view: 'code', file: 'src/main.ts', line: 7, folder: null });
    const depth = window.history.length;

    await act(async () => received.projects().onProjectLocationChange?.({ view: 'git' }));

    expect(window.location.pathname).toBe('/projects/proj-1/git');
    expect(window.location.search).toBe('');
    expect(window.history.length).toBe(depth + 1);
    expect(received.projects().projectLocation).toEqual({ view: 'git', file: null, line: null, folder: null });

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(received.projects().projectLocation?.view).toBe('code'));
  });

  it('follows the file the reader opens in the code view, a step Back undoes', async () => {
    await open('/projects/proj-1/code?file=README.md&line=7');
    const depth = window.history.length;

    await act(async () => received.projects().onProjectLocationChange?.({ view: 'code', file: 'src/other.ts', line: null }));

    expect(window.location.pathname).toBe('/projects/proj-1/code');
    expect(window.location.search).toBe('?file=src%2Fother.ts');
    expect(window.history.length).toBe(depth + 1);
    expect(received.projects().projectLocation).toEqual({ view: 'code', file: 'src/other.ts', line: null, folder: null });

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(received.projects().projectLocation).toEqual({ view: 'code', file: 'README.md', line: 7, folder: null }));
  });

  it('leaves the view to the project when the address names none, or one it does not know', async () => {
    await open('/projects/proj-1');
    expect(received.projects().projectLocation).toBeNull();

    await act(async () => { navigateAppTab('/projects/proj-1/nope'); });
    await waitFor(() => expect(window.location.pathname).toBe('/projects/proj-1/nope'));
    expect(received.projects().expandedId).toBe('proj-1');
    expect(received.projects().projectLocation).toBeNull();
  });

  it('returns to the bare list when the open project is closed', async () => {
    await open('/projects/proj-1');

    await act(async () => received.projects().onSetExpandedId(null));

    expect(received.projects().expandedId).toBeNull();
    expect(window.location.pathname).toBe('/projects');
  });

  it.each([
    ['discussions', '/discussions'],
    ['mcps', '/plugins'],
    ['workflows', '/workflows'],
    ['settings', '/config'],
  ])('navigates to %s', async (target, path) => {
    const ctx = await open('/projects');

    await act(async () => received.projects().onNavigate(target));

    expect(window.location.pathname).toBe(path);
    expect(ctx.toast).not.toHaveBeenCalled();
  });

  it('opens a plugin config at its address on the Plugins page', async () => {
    await open('/projects');

    await act(async () => received.projects().onNavigate('mcps:cfg-9'));

    expect(window.location.pathname).toBe('/plugins/cfg-9');
    expect(received.mcps().selectedConfigId).toBe('cfg-9');
  });

  it('opens a task at its address on the Planning page, or the backlog when none is named', async () => {
    await open('/projects');

    await act(async () => received.projects().onNavigate('planning:task-4'));
    expect(window.location.pathname).toBe('/planning/task-4');
    expect(received.planning().selectedTaskId).toBe('task-4');

    await act(async () => { window.history.back(); });
    await act(async () => received.projects().onNavigate('planning'));
    expect(window.location.pathname).toBe('/planning');
    expect(received.planning().selectedTaskId).toBeNull();
  });

  it('opens a discussion at its address, and runs the one it is asked to', async () => {
    await open('/projects');

    await act(async () => received.projects().onOpenDiscussion('disc-1'));
    expect(window.location.pathname).toBe('/discussions/disc-1');
    expect(received.discussions().autoRunDiscussionId).toBeNull();

    await act(async () => { window.history.back(); });
    await act(async () => received.projects().onAutoRunDiscussion('disc-2'));
    expect(window.location.pathname).toBe('/discussions/disc-2');
    expect(received.discussions().autoRunDiscussionId).toBe('disc-2');
  });
});

describe('Planning route', () => {
  it('reads the open task from its address, decoded', async () => {
    await open('/planning/task%2F4');

    expect(received.planning().selectedTaskId).toBe('task/4');
    expect(received.planning().initialSelectedTaskId).toBeUndefined();
  });

  it('moves the address when the reader picks a task, a step Back undoes', async () => {
    await open('/planning');
    const depth = window.history.length;

    act(() => received.planning().onSelectedTaskChange?.('task-4'));
    expect(window.location.pathname).toBe('/planning/task-4');
    expect(window.history.length).toBe(depth + 1);
    expect(received.planning().selectedTaskId).toBe('task-4');

    act(() => received.planning().onSelectedTaskChange?.(null));
    expect(window.location.pathname).toBe('/planning');
    expect(received.planning().selectedTaskId).toBeNull();

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(received.planning().selectedTaskId).toBe('task-4'));
  });

  it('leaves the address alone when the page confirms the task it names', async () => {
    await open('/planning/task-4');
    const depth = window.history.length;

    act(() => received.planning().onSelectedTaskChange?.('task-4'));

    expect(window.history.length).toBe(depth);
  });

  it('jumps to a discussion', async () => {
    await open('/planning/task-4');

    await act(async () => received.planning().onNavigateDiscussion?.('disc-5'));

    expect(window.location.pathname).toBe('/discussions/disc-5');
    expect(received.discussions().openDiscussionId).toBe('disc-5');
  });

  it('opens a discussion on one of its workspaces, once', async () => {
    await open('/planning/task-4');

    await act(async () => received.planning().onNavigateDiscussion?.('disc-5', { gitWorkspaceId: 'ws-2' }));

    expect(window.location.pathname).toBe('/discussions/disc-5');
    expect(received.discussions().gitWorkspaceTarget).toEqual({ discussionId: 'disc-5', workspaceId: 'ws-2' });
    await act(async () => received.discussions().onGitWorkspaceConsumed?.());
    expect(received.discussions().gitWorkspaceTarget).toBeNull();
  });
});

describe('Plugins route', () => {
  it('reads the open config from its address and lists only usable agents', async () => {
    await open('/plugins/cfg%2F9', makeContext({
      mcpOverviewLoaded: false,
      configLanguage: null,
      agents: [
        { agent_type: 'ClaudeCode', installed: true, runtime_available: true, enabled: true },
        { agent_type: 'Codex', installed: false, runtime_available: false, enabled: true },
      ] as DashboardOutletContext['agents'],
    }));
    const props = received.mcps();

    expect(props.selectedConfigId).toBe('cfg/9');
    expect(props.initialSelectedConfigId).toBeUndefined();
    expect(props.overviewLoaded).toBe(false);
    expect(props.configLanguage).toBeUndefined();
    expect(props.installedAgentTypes).toEqual(['ClaudeCode']);
  });

  it('moves the address when the reader picks or closes a config, a step Back undoes', async () => {
    await open('/plugins');
    const depth = window.history.length;

    act(() => received.mcps().onSelectedConfigChange?.('cfg-9', 'change'));
    expect(window.location.pathname).toBe('/plugins/cfg-9');
    expect(window.history.length).toBe(depth + 1);
    expect(received.mcps().selectedConfigId).toBe('cfg-9');

    act(() => received.mcps().onSelectedConfigChange?.(null, 'change'));
    expect(window.location.pathname).toBe('/plugins');

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(received.mcps().selectedConfigId).toBe('cfg-9'));
  });

  it('leaves the address alone when the page confirms the config it names', async () => {
    await open('/plugins/cfg-9');
    const depth = window.history.length;

    act(() => received.mcps().onSelectedConfigChange?.('cfg-9', 'change'));

    expect(window.history.length).toBe(depth);
  });

  it('replaces the address when the page lets go of a config the list does not know', async () => {
    await open('/plugins/gone');
    const depth = window.history.length;

    act(() => received.mcps().onSelectedConfigChange?.(null, 'restore'));

    expect(window.location.pathname).toBe('/plugins');
    expect(window.history.length).toBe(depth);
  });
});

describe('Workflows route', () => {
  it.each([
    ['/workflows', { tab: 'workflows', resourceId: null, runId: null }],
    ['/workflows/wf%2F1', { tab: 'workflows', resourceId: 'wf/1', runId: null }],
    ['/workflows/wf-1/runs/run%202', { tab: 'workflows', resourceId: 'wf-1', runId: 'run 2' }],
    ['/workflows/qp', { tab: 'quickPrompts', resourceId: null, runId: null }],
    ['/workflows/qp/qp-1', { tab: 'quickPrompts', resourceId: 'qp-1', runId: null }],
    ['/workflows/qa/qa-1', { tab: 'quickApis', resourceId: 'qa-1', runId: null }],
    ['/workflows/qe/qe-1', { tab: 'quickExecs', resourceId: 'qe-1', runId: null }],
    ['/workflows/skills', { tab: 'skills', resourceId: null, runId: null }],
    ['/workflows/skills/skill-1', { tab: 'skills', resourceId: 'skill-1', runId: null }],
    ['/workflows/new', { tab: 'workflows', resourceId: null, runId: null, editor: 'create' }],
    ['/workflows/wf-1/edit', { tab: 'workflows', resourceId: 'wf-1', runId: null, editor: 'edit' }],
  ])('reads %s as the selection', async (path, selection) => {
    await open(path);

    expect(received.workflows().selection).toEqual(selection);
    expect(window.location.pathname).toBe(path);
  });

  it('keeps one page instance across the shapes of its address', async () => {
    // Every shape is its own route; the page must not remount between them,
    // or its lists, editors and scroll positions would be lost on each click.
    await open('/workflows/qp/qp-1');

    act(() => received.workflows().onSelectionChange?.({ tab: 'workflows', resourceId: 'wf-1', runId: 'run-2' }, 'change'));
    expect(window.location.pathname).toBe('/workflows/wf-1/runs/run-2');
    act(() => received.workflows().onSelectionChange?.({ tab: 'skills', resourceId: null }, 'change'));
    expect(window.location.pathname).toBe('/workflows/skills');
    act(() => received.workflows().onSelectionChange?.({ tab: 'workflows', resourceId: null, editor: 'create' }, 'change'));
    expect(window.location.pathname).toBe('/workflows/new');
    act(() => received.workflows().onSelectionChange?.({ tab: 'workflows', resourceId: 'wf-1', editor: 'edit' }, 'change'));
    expect(window.location.pathname).toBe('/workflows/wf-1/edit');

    expect(page.mounts.workflows).toBe(1);
  });

  it('moves the address when the reader changes tab or resource, a step Back undoes', async () => {
    await open('/workflows');
    const depth = window.history.length;

    act(() => received.workflows().onSelectionChange?.({ tab: 'quickPrompts', resourceId: 'qp-1', runId: null }, 'change'));
    expect(window.location.pathname).toBe('/workflows/qp/qp-1');
    expect(window.history.length).toBe(depth + 1);

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(received.workflows().selection).toEqual({ tab: 'workflows', resourceId: null, runId: null }));
  });

  it('lets what the page restores or lets go on its own replace the address', async () => {
    await open('/workflows');
    const depth = window.history.length;

    act(() => received.workflows().onSelectionChange?.({ tab: 'skills', resourceId: 'skill-1', runId: null }, 'restore'));

    expect(window.location.pathname).toBe('/workflows/skills/skill-1');
    expect(window.history.length).toBe(depth);

    act(() => received.workflows().onSelectionChange?.({ tab: 'skills', resourceId: null, runId: null }, 'restore'));

    expect(window.location.pathname).toBe('/workflows/skills');
    expect(window.history.length).toBe(depth);
  });

  it('reopens the bare address where the previous visit left off, in place of it', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'quickPrompts', resourceId: 'qp-1' }));
    const depth = window.history.length;
    await open('/workflows');

    await waitFor(() => expect(window.location.pathname).toBe('/workflows/qp/qp-1'));
    expect(window.history.length).toBe(depth);
    expect(received.workflows().selection).toEqual({ tab: 'quickPrompts', resourceId: 'qp-1', runId: null });
    localStorage.removeItem('kronn:automationNavigation');
  });

  it('leaves the bare address alone when the previous visit was the workflows list', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'workflows', resourceId: null }));
    const depth = window.history.length;
    await open('/workflows');

    expect(window.location.pathname).toBe('/workflows');
    expect(window.history.length).toBe(depth);
    expect(received.workflows().selection).toEqual({ tab: 'workflows', resourceId: null, runId: null });
    localStorage.removeItem('kronn:automationNavigation');
  });

  it('lets an explicit address, and Back to it, win over the previous visit', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'quickPrompts', resourceId: 'qp-1' }));
    await open('/workflows/qa/qa-1');
    expect(window.location.pathname).toBe('/workflows/qa/qa-1');
    expect(received.workflows().selection).toEqual({ tab: 'quickApis', resourceId: 'qa-1', runId: null });

    act(() => received.workflows().onSelectionChange?.({ tab: 'skills', resourceId: null, runId: null }, 'change'));
    expect(window.location.pathname).toBe('/workflows/skills');

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(window.location.pathname).toBe('/workflows/qa/qa-1'));
    expect(received.workflows().selection).toEqual({ tab: 'quickApis', resourceId: 'qa-1', runId: null });
    localStorage.removeItem('kronn:automationNavigation');
  });

  it('opens the wizard on the workflows list when a preset arrives at the bare address', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'quickPrompts', resourceId: 'qp-1' }));
    await open('/discussions');
    await act(async () => received.discussions().onLaunchWorkflowFromPreset?.('ticket-to-pr', 'proj-1'));

    expect(window.location.pathname).toBe('/workflows');
    expect(received.workflows().selection).toEqual({ tab: 'workflows', resourceId: null, runId: null });
    expect(received.workflows().pendingPreset).toEqual({ presetId: 'ticket-to-pr', projectId: 'proj-1' });
    localStorage.removeItem('kronn:automationNavigation');
  });

  it('leaves the address alone when the page confirms what it names', async () => {
    await open('/workflows/qa/qa-1');
    const depth = window.history.length;

    act(() => received.workflows().onSelectionChange?.({ tab: 'quickApis', resourceId: 'qa-1' }, 'change'));

    expect(window.location.pathname).toBe('/workflows/qa/qa-1');
    expect(window.history.length).toBe(depth);
  });

  it('hands the wizard preset it was sent with, then consumes it', async () => {
    await open('/discussions');
    await act(async () => received.discussions().onLaunchWorkflowFromPreset?.('ticket-to-pr', 'proj-1'));
    expect(window.location.pathname).toBe('/workflows');
    expect(received.workflows().pendingPreset).toEqual({ presetId: 'ticket-to-pr', projectId: 'proj-1' });
    const depth = window.history.length;

    act(() => received.workflows().onPendingPresetConsumed?.());

    expect(received.workflows().pendingPreset).toBeNull();
    expect(window.history.length).toBe(depth);
    expect(window.history.state?.usr ?? null).toBeNull();
  });

  it('lands on the first child of a launched batch and focuses its group', async () => {
    const ctx = await open('/workflows');

    await act(async () => received.workflows().onBatchLaunched?.(['disc-a', 'disc-b'], 'batch-1', 'compare'));

    expect(ctx.markBatchSending).toHaveBeenCalledWith(['disc-a', 'disc-b']);
    expect(ctx.refetchDiscussions).toHaveBeenCalledOnce();
    expect(window.location.pathname).toBe('/discussions/disc-a');
    expect(received.discussions().openDiscussionId).toBe('disc-a');
    expect(received.discussions().focusBatchId).toBe('batch-1');
    expect(received.discussions().focusBatchMode).toBe('compare');
  });

  it('stays put when a launched batch created no discussion', async () => {
    const ctx = await open('/workflows');

    await act(async () => received.workflows().onBatchLaunched?.([], 'batch-1'));

    expect(ctx.refetchDiscussions).toHaveBeenCalledOnce();
    expect(window.location.pathname).toBe('/workflows');
  });

  it.each([
    ['a batch group', (p: ComponentProps<typeof WorkflowsPage>) => p.onNavigateToBatch?.('batch-1'), '/discussions'],
    ['a discussion to run', (p: ComponentProps<typeof WorkflowsPage>) => p.onNavigateDiscussion?.('disc-1'), '/discussions/disc-1'],
    ['an Artifact', (p: ComponentProps<typeof WorkflowsPage>) => p.onNavigatePage?.('page-7'), '/pages/page-7'],
    ['Plugins', (p: ComponentProps<typeof WorkflowsPage>) => p.onNavigateMcp?.(), '/plugins'],
    ['Settings', (p: ComponentProps<typeof WorkflowsPage>) => p.onNavigateSettings?.(), '/config'],
  ])('jumps to %s', async (_label, jump, path) => {
    await open('/workflows');

    await act(async () => jump(received.workflows()));

    expect(window.location.pathname).toBe(path);
  });

  it('focuses a batch as a batch, and runs the discussion it jumps to', async () => {
    await open('/workflows');

    await act(async () => received.workflows().onNavigateToBatch?.('batch-1'));
    expect(received.discussions().focusBatchId).toBe('batch-1');
    expect(received.discussions().focusBatchMode).toBe('batch');
    expect(received.discussions().openDiscussionId).toBeNull();

    await act(async () => { window.history.back(); });
    await act(async () => received.workflows().onNavigateDiscussion?.('disc-1'));
    expect(received.discussions().autoRunDiscussionId).toBe('disc-1');
    expect(received.discussions().openDiscussionId).toBeNull();
  });
});

describe('Discussions route', () => {
  it('reads the open discussion and its message from the address', async () => {
    await open('/discussions/disc%2F1?message=msg%2F2');
    const props = received.discussions();

    expect(props.initialActiveDiscussionId).toBe('disc/1');
    expect(props.openDiscussionId).toBe('disc/1');
    expect(props.initialMessageId).toBe('msg/2');
    expect(props.autoRunDiscussionId).toBeNull();
    expect(props.focusBatchId).toBeNull();
    expect(props.focusBatchMode).toBe('batch');
  });

  it('reopens where the previous visit left off from the bare address', async () => {
    await open('/discussions', makeContext({ restorableDiscussionId: 'disc-42' }));
    const props = received.discussions();

    expect(props.initialActiveDiscussionId).toBe('disc-42');
    // Only an address names a message; a checkpoint never does.
    expect(props.initialMessageId).toBeNull();
    expect(props.openDiscussionId).toBeNull();
  });

  it('hands an id that cannot be decoded to the page as it is, like any unknown id', async () => {
    await open('/discussions/%E0%A4%A');

    expect(window.location.pathname).toBe('/discussions/%E0%A4%A');
    expect(received.discussions().openDiscussionId).toBe('%E0%A4%A');
    expect(received.discussions().initialActiveDiscussionId).toBe('%E0%A4%A');
  });

  it('names no message without a discussion', async () => {
    await open('/discussions?message=msg-2');

    expect(received.discussions().initialMessageId).toBeNull();
  });

  it('lets the discussion the page restores on its own replace the bare address', async () => {
    const ctx = await open('/discussions', makeContext({ restorableDiscussionId: 'disc-42' }));
    const depth = window.history.length;

    act(() => received.discussions().onActiveDiscussionChange('disc-42'));

    expect(ctx.setActiveDiscussionId).toHaveBeenCalledWith('disc-42');
    expect(window.location.pathname).toBe('/discussions/disc-42');
    expect(window.history.length).toBe(depth);
  });

  it('moves the address when the reader picks a discussion, a step Back undoes', async () => {
    await open('/discussions/disc-1');
    const depth = window.history.length;

    act(() => received.discussions().onActiveDiscussionChange('disc-2'));
    expect(window.location.pathname).toBe('/discussions/disc-2');
    expect(window.history.length).toBe(depth + 1);
    expect(received.discussions().openDiscussionId).toBe('disc-2');

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(received.discussions().openDiscussionId).toBe('disc-1'));
  });

  it('leaves the address alone when the page confirms the discussion it names', async () => {
    await open('/discussions/disc-1');
    const depth = window.history.length;

    act(() => received.discussions().onActiveDiscussionChange('disc-1'));

    expect(window.location.pathname).toBe('/discussions/disc-1');
    expect(window.history.length).toBe(depth);
  });

  it('returns to the bare list when the open discussion is closed', async () => {
    await open('/discussions/disc-1');

    act(() => received.discussions().onActiveDiscussionChange(null));

    expect(window.location.pathname).toBe('/discussions');
    expect(received.discussions().openDiscussionId).toBeNull();
  });

  it('consumes an arrival intent by replacing the entry without it', async () => {
    await open('/projects');
    await act(async () => received.projects().onAutoRunDiscussion('disc-2'));
    expect(received.discussions().autoRunDiscussionId).toBe('disc-2');
    const depth = window.history.length;

    act(() => received.discussions().onAutoRunConsumed?.());

    expect(received.discussions().autoRunDiscussionId).toBeNull();
    expect(received.discussions().openDiscussionId).toBe('disc-2');
    expect(window.location.pathname).toBe('/discussions/disc-2');
    expect(window.history.length).toBe(depth);
    expect(window.history.state?.usr ?? null).toBeNull();
  });

  it('acknowledges a consumed prefill through the shell', async () => {
    const ctx = await open('/discussions');

    act(() => received.discussions().onPrefillConsumed?.());

    expect(ctx.setDiscPrefill).toHaveBeenCalledWith(null);
  });

  it('keeps the identity of its acknowledgements across shell renders', async () => {
    // The page lists them in effect dependencies: a new identity on every
    // poll or stream chunk would re-run those effects.
    const ctx = await open('/discussions');
    const before = received.discussions();

    act(() => shell.update({ ...ctx, sendingMap: { 'disc-1': true } }));

    const after = received.discussions();
    expect(after.sendingMap).toEqual({ 'disc-1': true });
    expect(after.onPrefillConsumed).toBe(before.onPrefillConsumed);
    expect(after.onAutoRunConsumed).toBe(before.onAutoRunConsumed);
    expect(after.onOpenDiscConsumed).toBe(before.onOpenDiscConsumed);
    expect(after.onFocusBatchConsumed).toBe(before.onFocusBatchConsumed);
    expect(after.onActiveDiscussionChange).toBe(before.onActiveDiscussionChange);
  });

  it('gives a comparison its own address, and keeps it while the page names the discussion behind it', async () => {
    await open('/discussions/compare/run%201');
    expect(received.discussions().compareRunId).toBe('run 1');
    expect(received.discussions().openDiscussionId).toBeNull();

    // The page restores the discussion it keeps behind the comparison: not the address.
    await act(async () => received.discussions().onActiveDiscussionChange?.('disc-1'));
    expect(window.location.pathname).toBe('/discussions/compare/run%201');

    // Closing the comparison returns to the discussions.
    await act(async () => received.discussions().onCompareChange?.(null));
    expect(window.location.pathname).toBe('/discussions');
    expect(received.discussions().compareRunId).toBeNull();
  });

  it('moves to a comparison the reader opens, a step Back undoes', async () => {
    await open('/discussions/disc-1');
    const depth = window.history.length;

    await act(async () => received.discussions().onCompareChange?.('run-2'));
    expect(window.location.pathname).toBe('/discussions/compare/run-2');
    expect(window.history.length).toBe(depth + 1);

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(window.location.pathname).toBe('/discussions/disc-1'));
  });

  it('opens an improved Quick Prompt at its address, flashed on arrival', async () => {
    await open('/discussions/disc-1');

    await act(async () => received.discussions().onNavigate('workflows', { quickPromptId: 'qp-7' }));

    expect(window.location.pathname).toBe('/workflows/qp/qp-7');
    expect(received.workflows().highlightQuickPromptId).toBe('qp-7');
    // Acknowledged: a reload or a Back never flashes it again.
    await act(async () => received.workflows().onHighlightConsumed?.());
    expect(received.workflows().highlightQuickPromptId).toBeNull();
    expect(window.location.pathname).toBe('/workflows/qp/qp-7');
  });

  it('jumps to a project at its own address', async () => {
    await open('/discussions');

    await act(async () => received.discussions().onNavigate('projects', { projectId: 'proj-1' }));

    expect(window.location.pathname).toBe('/projects/proj-1');
  });

  it('does not move when the page asked for is Discussions, even with a project named', async () => {
    // The dev kickoff (after « issues created ») prefills a discussion for its
    // project and asks for Discussions, where it already is: as before pages
    // had addresses, the form opens over the discussion, whose address stays.
    await open('/discussions/disc-1');
    const depth = window.history.length;

    await act(async () => received.discussions().onNavigate('discussions', { projectId: 'proj-1' }));

    expect(window.location.pathname).toBe('/discussions/disc-1');
    expect(window.history.length).toBe(depth);
  });

  it('points Configuration at the agent tier a model error names, once', async () => {
    await open('/discussions/disc-1');

    await act(async () => received.discussions().onNavigate('settings', {
      scrollTo: 'settings-agent-config',
      modelTier: { agentType: 'LiteLlm', tier: 'default' },
    }));

    expect(window.location.pathname).toBe('/config');
    expect(window.location.hash).toBe('#settings-agent-config');
    expect(received.settings().modelTierTarget).toEqual({ agentType: 'LiteLlm', tier: 'default' });
    // Acknowledged: a reload or a Back never points at it again.
    await act(async () => received.settings().onModelTierTargetConsumed?.());
    expect(received.settings().modelTierTarget).toBeNull();
    expect(window.location.hash).toBe('#settings-agent-config');
  });

  it('jumps to a Configuration section at its own address, scrolled into view on arrival', async () => {
    await open('/discussions');
    const anchor = document.createElement('div');
    anchor.id = 'settings-server';
    anchor.scrollIntoView = vi.fn();
    document.body.appendChild(anchor);
    try {
      await act(async () => received.discussions().onNavigate('settings', { scrollTo: 'settings-server' }));
      expect(window.location.pathname).toBe('/config');
      expect(window.location.hash).toBe('#settings-server');

      await waitFor(() => expect(anchor.scrollIntoView).toHaveBeenCalledWith({ behavior: 'smooth', block: 'start' }));
    } finally {
      anchor.remove();
    }
  });

  it('waits for a section that renders late, then scrolls to it', async () => {
    await open('/config#settings-late');
    const anchor = document.createElement('div');
    anchor.id = 'settings-late';
    anchor.scrollIntoView = vi.fn();
    // Appears a few frames after the page, like a section fed by a request.
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 50)); });
    document.body.appendChild(anchor);
    try {
      await waitFor(() => expect(anchor.scrollIntoView).toHaveBeenCalledTimes(1));
    } finally {
      anchor.remove();
    }
  });

  it('opens the parent workflow of a batch, and a preset in the wizard', async () => {
    await open('/discussions');

    await act(async () => received.discussions().onNavigate('workflows', { workflowId: 'wf-1' }));
    expect(window.location.pathname).toBe('/workflows/wf-1');
    expect(received.workflows().selection).toEqual({ tab: 'workflows', resourceId: 'wf-1', runId: null });

    await act(async () => { window.history.back(); });
    await act(async () => received.discussions().onLaunchWorkflowFromPreset?.('ticket-to-pr', 'proj-1'));
    expect(window.location.pathname).toBe('/workflows');
    expect(received.workflows().pendingPreset).toEqual({ presetId: 'ticket-to-pr', projectId: 'proj-1' });
  });
});

describe('Settings route', () => {
  it('jumps to a discussion and resets through the shell', async () => {
    const ctx = await open('/config');

    received.settings().onReset();
    expect(ctx.onReset).toHaveBeenCalledOnce();

    await act(async () => received.settings().onNavigateDiscussion?.('disc-1'));
    expect(window.location.pathname).toBe('/discussions/disc-1');
  });

  it('shows the API audit section only once an API plugin is configured', async () => {
    await open('/config');
    expect(received.settings().hasConfiguredApi).toBe(false);
    cleanup();

    const server = (id: string, api_spec: unknown) => ({ id, api_spec });
    await open('/config', makeContext({
      mcpOverview: {
        ...emptyOverview,
        servers: [server('mcp-only', null), server('api-plugin', {})] as unknown as McpOverview['servers'],
        configs: [{ id: 'cfg-1', server_id: 'api-plugin' }] as unknown as McpOverview['configs'],
      },
    }));
    expect(received.settings().hasConfiguredApi).toBe(true);
    cleanup();

    await open('/config', makeContext({
      mcpOverview: {
        ...emptyOverview,
        servers: [server('mcp-only', null), server('api-plugin', {})] as unknown as McpOverview['servers'],
        configs: [{ id: 'cfg-1', server_id: 'mcp-only' }] as unknown as McpOverview['configs'],
      },
    }));
    expect(received.settings().hasConfiguredApi).toBe(false);
  });
});

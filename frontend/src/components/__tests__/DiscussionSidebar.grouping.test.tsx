// DiscussionSidebar — grouping / collapse / search / batch-pastille coverage.
//
// The contact-add, markAllRead and source-badge suites already exercise
// the header + cross-agent paths. This suite targets the previously
// uncovered render branches:
//   - discussions grouped by project + org headers (multi-org → multiple
//     headers, "Local" sorts last)
//   - global (no-project) group rendering + collapse toggle fires
//     onToggleGroup with the right key
//   - project group collapse hides its discs, expand shows them
//   - pinned / Favorites cross-project section (ordering + always visible)
//   - search filter narrows the list (title + id-prefix), clears, and the
//     clear button resets it; no-match shows nothing
//   - the PROJECT_LOOSE_LIMIT "+N more" expander
//   - batch group folder: pastille (status pill), parent-workflow pill
//     navigation, delete + retry confirm flows
//   - selecting a discussion fires onSelect
//
// projectsApi.discSources is mocked (empty) via the shared apiMock so the
// source-filter dropdown stays hidden and the useEffect resolves cleanly.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { projects as projectsApi } from '../../lib/api';
import { DiscussionSidebar } from '../DiscussionSidebar';
import type { Discussion, Project, BatchRunSummary } from '../../types/generated';

const noop = () => {};

const baseProps = {
  discussions: [] as Discussion[],
  projects: [] as Project[],
  activeId: null as string | null,
  sendingMap: {} as Record<string, boolean>,
  lastSeenMsgCount: {} as Record<string, number>,
  contacts: [],
  contactsOnline: {},
  wsConnected: true,
  isMobile: false,
  onSelect: noop,
  onArchive: noop,
  onUnarchive: noop,
  onDelete: noop,
  onTogglePin: noop,
  onNewDiscussion: noop,
  onClose: noop,
  onContactAdd: vi.fn().mockResolvedValue(undefined),
  onContactDelete: vi.fn().mockResolvedValue(undefined),
  toast: vi.fn(),
  t: (key: string, ...args: (string | number)[]) =>
    args.length > 0 ? `${key}:${args.join(',')}` : key,
  collapsedGroups: new Set<string>(),
  onToggleGroup: noop,
};

const mkDisc = (over: Partial<Discussion> & { id: string }): Discussion => ({
  project_id: null,
  title: `Discussion ${over.id}`,
  agent: 'ClaudeCode',
  language: 'fr',
  participants: ['ClaudeCode'],
  messages: [],
  message_count: 0,
  non_system_message_count: 0, tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
  archived: false,
  pinned: false, pin_first_message: false,
  workspace_mode: 'Direct',
  created_at: '2026-05-15T10:00:00Z',
  updated_at: '2026-05-15T10:00:00Z',
  awaiting_agent: false,
  ...over,
});

const mkProject = (id: string, name: string, repo_url: string | null = null): Project => ({
  id,
  name,
  path: `/repos/${name}`,
  repo_url,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'NoTemplate',
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-05-15T10:00:00Z',
  updated_at: '2026-05-15T10:00:00Z',
});

const mkBatchSummary = (over: Partial<BatchRunSummary> & { run_id: string }): BatchRunSummary => ({
  batch_name: null,
  batch_total: 1,
  status: 'Completed' as BatchRunSummary['status'],
  quick_prompt_id: null,
  quick_prompt_name: null,
  quick_prompt_icon: null,
  parent_run_id: null,
  parent_workflow_id: null,
  parent_workflow_name: null,
  parent_run_sequence: null,
  ...over,
});

beforeEach(() => {
  vi.clearAllMocks();
  // No source bindings → source-filter dropdown stays hidden, useEffect
  // resolves with an empty list. mockResolvedValue (not Once) so the
  // effect re-fire on discussions.length change still resolves.
  (projectsApi.discSources as ReturnType<typeof vi.fn>).mockResolvedValue([]);
});

describe('DiscussionSidebar — grouping', () => {
  it('renders one org header per distinct org, "Local" sorts last', async () => {
    const projects = [
      mkProject('p-acme', 'AcmeRepo', 'git@github.com:acme-org/AcmeRepo.git'),
      mkProject('p-zeta', 'ZetaRepo', 'git@github.com:zeta-org/ZetaRepo.git'),
      mkProject('p-local', 'LocalRepo', null), // no repo_url → "Local" group
    ];
    const discussions = [
      mkDisc({ id: 'd-acme', project_id: 'p-acme', title: 'Acme thread' }),
      mkDisc({ id: 'd-zeta', project_id: 'p-zeta', title: 'Zeta thread' }),
      mkDisc({ id: 'd-local', project_id: 'p-local', title: 'Local thread' }),
    ];

    render(<DiscussionSidebar {...baseProps} projects={projects} discussions={discussions} />);

    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    // Three distinct orgs → three org headers. The `t('disc.local')` mock
    // returns the key "disc.local" for the no-repo group.
    const orgHeaders = document.querySelectorAll('.disc-org-header');
    expect(orgHeaders).toHaveLength(3);
    const orgTexts = Array.from(orgHeaders).map(h => h.textContent ?? '');
    expect(orgTexts.some(t => t.includes('acme-org'))).toBe(true);
    expect(orgTexts.some(t => t.includes('zeta-org'))).toBe(true);
    // "Local" (disc.local) sorts last.
    expect(orgTexts[orgTexts.length - 1]).toContain('disc.local');

    // Each project folder + its disc render.
    expect(screen.getByText('Acme thread')).toBeInTheDocument();
    expect(screen.getByText('Zeta thread')).toBeInTheDocument();
    expect(screen.getByText('Local thread')).toBeInTheDocument();
  });

  it('renders the global (no-project) group and toggles it via onToggleGroup', async () => {
    const onToggleGroup = vi.fn();
    const discussions = [
      mkDisc({ id: 'g1', project_id: null, title: 'Global one' }),
    ];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        onToggleGroup={onToggleGroup}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    expect(screen.getByText('Global one')).toBeInTheDocument();
    // The global group header uses key '__global__'.
    const globalBtn = screen.getByText('disc.noProject').closest('button')!;
    expect(globalBtn).toHaveAttribute('aria-expanded', 'true');
    fireEvent.click(globalBtn);
    expect(onToggleGroup).toHaveBeenCalledWith('__global__');
  });

  it('a collapsed group key hides its discs (no search override)', async () => {
    const discussions = [
      mkDisc({ id: 'g1', project_id: null, title: 'Hidden global thread' }),
    ];
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        collapsedGroups={new Set(['__global__'])}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    // Header still renders, but the disc inside is collapsed away.
    expect(screen.getByText('disc.noProject')).toBeInTheDocument();
    expect(screen.queryByText('Hidden global thread')).toBeNull();
    // Collapsed header reports aria-expanded=false.
    const globalBtn = screen.getByText('disc.noProject').closest('button')!;
    expect(globalBtn).toHaveAttribute('aria-expanded', 'false');
  });

  it('collapses the complete canonical tree behind the Projects section', async () => {
    const onToggleGroup = vi.fn();
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={[mkDisc({ id: 'g1', project_id: null, title: 'Canonical thread' })]}
        collapsedGroups={new Set(['__projects__'])}
        onToggleGroup={onToggleGroup}
      />,
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const projectsButton = screen.getByText('projects.title').closest('button')!;
    expect(projectsButton).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText('Canonical thread')).toBeNull();
    fireEvent.click(projectsButton);
    expect(onToggleGroup).toHaveBeenCalledWith('__projects__');
  });

  it('clicking a project folder fires onToggleGroup with the project id', async () => {
    const onToggleGroup = vi.fn();
    const projects = [mkProject('p1', 'ProjectAlpha')];
    const discussions = [mkDisc({ id: 'd1', project_id: 'p1', title: 'Alpha thread' })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={discussions}
        onToggleGroup={onToggleGroup}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const folderBtn = screen.getByText('ProjectAlpha').closest('button')!;
    fireEvent.click(folderBtn);
    expect(onToggleGroup).toHaveBeenCalledWith('p1');
  });

  it('selecting a discussion fires onSelect with the non-System message count', async () => {
    const onSelect = vi.fn();
    // The selection basis is `unseenBasis` → `non_system_message_count`, NOT the
    // raw `message_count` (which is inflated by tool-log / summary / refusal
    // System rows). Here the disc has 12 raw rows but only 7 user/agent ones.
    const discussions = [mkDisc({
      id: 'd1', project_id: null, title: 'Click me',
      message_count: 12, non_system_message_count: 7, tier: "default" as const, summary_strategy: "Auto" as const, introspection_call_count: 0,
    })];
    render(<DiscussionSidebar {...baseProps} discussions={discussions} onSelect={onSelect} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    // The row is a native button: its click path is shared by pointer and
    // keyboard activation (the browser synthesizes click for Enter/Space).
    const row = screen.getByRole('button', { name: /Click me/ });
    fireEvent.click(row);
    expect(onSelect).toHaveBeenCalledWith('d1', 7);
  });
});

describe('DiscussionSidebar — pinned / favorites section', () => {
  it('renders pinned discs cross-project, sorted by updated_at desc', async () => {
    const discussions = [
      mkDisc({ id: 'p-old', pinned: true, pin_first_message: false, title: 'Pinned older', updated_at: '2026-05-10T00:00:00Z' }),
      mkDisc({ id: 'p-new', pinned: true, pin_first_message: false, title: 'Pinned newer', updated_at: '2026-05-20T00:00:00Z' }),
      mkDisc({ id: 'reg', pinned: false, pin_first_message: false, title: 'Regular' }),
    ];
    render(<DiscussionSidebar {...baseProps} discussions={discussions} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    // Favorites header renders with the pinned count.
    const favHeader = screen.getByText('disc.favorites');
    expect(favHeader).toBeInTheDocument();

    // Scope to the favorites <div> (header + its pinned items). The same
    // pinned disc also renders in its project/global group via a `pin-<id>`
    // vs plain key, so a global getByText would match twice — we query
    // within the favorites container instead.
    const favSection = favHeader.closest('.disc-group-btn')!.parentElement as HTMLElement;
    const newer = within(favSection).getByText('Pinned newer');
    const older = within(favSection).getByText('Pinned older');
    expect(newer).toBeInTheDocument();
    expect(older).toBeInTheDocument();
    // DOM order inside Favorites: newer precedes older (sort desc by updated_at).
    expect(newer.compareDocumentPosition(older) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('no favorites header when nothing is pinned', async () => {
    const discussions = [mkDisc({ id: 'reg', pinned: false, pin_first_message: false, title: 'Regular' })];
    render(<DiscussionSidebar {...baseProps} discussions={discussions} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());
    expect(screen.queryByText('disc.favorites')).toBeNull();
  });

  it('favorites section is collapsible like any other group', async () => {
    const onToggleGroup = vi.fn();
    const discussions = [
      mkDisc({ id: 'p1', project_id: null, pinned: true, pin_first_message: false, title: 'Pinned thread' }),
    ];

    // Expanded by default: header click reports the toggle upward.
    const { unmount } = render(
      <DiscussionSidebar {...baseProps} discussions={discussions} onToggleGroup={onToggleGroup} />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());
    fireEvent.click(screen.getByText('disc.favorites'));
    expect(onToggleGroup).toHaveBeenCalledWith('__favorites__');
    unmount();

    // Collapsed: header stays, pinned item hidden inside Favorites (it
    // still renders once in its own group below — hence queryAllByText).
    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        collapsedGroups={new Set(['__favorites__'])}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());
    expect(screen.getByText('disc.favorites')).toBeInTheDocument();
    expect(screen.queryAllByText('Pinned thread')).toHaveLength(1);
  });
});

describe('DiscussionSidebar — global search entry point', () => {
  const discussions = [
    mkDisc({ id: 'aabbccdd', project_id: null, title: 'Apple pie recipe' }),
    mkDisc({ id: 'eeff0011', project_id: null, title: 'Banana bread' }),
  ];

  it('keeps the local tree stable while the user composes a global query', async () => {
    render(<DiscussionSidebar {...baseProps} discussions={discussions} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const input = document.querySelector('.disc-search-input') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'banana' } });

    // The field searches the backend on Enter. It must not remount/filter the
    // potentially huge local tree on every keystroke.
    expect(screen.getByText('Apple pie recipe')).toBeInTheDocument();
    expect(screen.getByText('Banana bread')).toBeInTheDocument();

    // Clear resets the shared query without changing the list.
    const clearBtn = screen.getByLabelText('disc.searchClear');
    fireEvent.click(clearBtn);
    expect(input.value).toBe('');
    expect(screen.getByText('Apple pie recipe')).toBeInTheDocument();
    expect(screen.getByText('Banana bread')).toBeInTheDocument();
  });

  it('uses one query when switching from quick to advanced search', async () => {
    const onOpenGlobalSearch = vi.fn();
    const { rerender } = render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        onOpenGlobalSearch={onOpenGlobalSearch}
      />,
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const quickInput = document.querySelector('.disc-search-input') as HTMLInputElement;
    fireEvent.change(quickInput, { target: { value: 'banana' } });
    expect(screen.getAllByRole('textbox')).toHaveLength(1);
    expect(screen.getByTestId('disc-open-global-search')).toBeVisible();
    fireEvent.keyDown(quickInput, { key: 'Enter' });
    expect(onOpenGlobalSearch).toHaveBeenCalledTimes(1);

    rerender(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        onOpenGlobalSearch={onOpenGlobalSearch}
        globalSearchOpen
        globalSearchAuthors={['Codex']}
        onCloseGlobalSearch={noop}
        onOpenGlobalSearchResult={noop}
      />,
    );

    const advancedInput = screen.getByTestId('global-search-input') as HTMLInputElement;
    expect(advancedInput.value).toBe('banana');
    expect(document.querySelector('.disc-search-input')).not.toBeVisible();
  });

  it('clears the shared query when advanced search is closed', async () => {
    const onCloseGlobalSearch = vi.fn();
    const { rerender } = render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        onOpenGlobalSearch={noop}
      />,
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const quickInput = document.querySelector('.disc-search-input') as HTMLInputElement;
    fireEvent.change(quickInput, { target: { value: 'banana' } });

    rerender(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        onOpenGlobalSearch={noop}
        globalSearchOpen
        globalSearchAuthors={['Codex']}
        onCloseGlobalSearch={onCloseGlobalSearch}
        onOpenGlobalSearchResult={noop}
      />,
    );
    fireEvent.click(screen.getByLabelText('disc.globalSearch.close'));
    expect(onCloseGlobalSearch).toHaveBeenCalledTimes(1);

    rerender(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        onOpenGlobalSearch={noop}
      />,
    );
    expect((screen.getByPlaceholderText('disc.globalSearch.placeholder') as HTMLInputElement).value)
      .toBe('');
    expect(screen.getByText('Apple pie recipe')).toBeInTheDocument();
    expect(screen.getByText('Banana bread')).toBeInTheDocument();
  });

});

describe('DiscussionSidebar — high-volume smart sections', () => {
  it('shows unique Follow up / Favorites / Recent shortcuts above the canonical tree', async () => {
    const discussions = Array.from({ length: 20 }, (_, index) => mkDisc({
      id: `smart-${index}`,
      title: `Smart discussion ${index}`,
      pinned: index === 0 || index === 1,
      non_system_message_count: index === 2 ? 3 : 0,
      message_count: index === 2 ? 3 : 0,
      updated_at: `2026-07-${String(20 + Math.min(index, 9)).padStart(2, '0')}T10:00:00Z`,
    }));

    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        sendingMap={{ 'smart-1': true }}
      />,
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const followUp = document.querySelector('.disc-sidebar-follow-up') as HTMLElement;
    const favorites = document.querySelector('.disc-sidebar-favorites') as HTMLElement;
    const recent = document.querySelector('.disc-sidebar-recent') as HTMLElement;
    expect(followUp).not.toBeNull();
    expect(favorites).not.toBeNull();
    expect(recent).not.toBeNull();

    // Running wins over pinned, unread joins Follow up, and neither leaks into
    // another smart section. The canonical project/general tree still contains
    // every discussion below those shortcuts.
    expect(within(followUp).getByText('Smart discussion 1')).toBeInTheDocument();
    expect(within(followUp).getByText('Smart discussion 2')).toBeInTheDocument();
    expect(within(favorites).queryByText('Smart discussion 1')).toBeNull();
    expect(within(favorites).getByText('Smart discussion 0')).toBeInTheDocument();
    expect(within(recent).queryByText('Smart discussion 0')).toBeNull();
    expect(within(recent).queryByText('Smart discussion 1')).toBeNull();
    expect(within(recent).queryByText('Smart discussion 2')).toBeNull();
  });

  it('caps shortcut rows until the user explicitly asks for the rest', async () => {
    const discussions = Array.from({ length: 20 }, (_, index) => mkDisc({
      id: `unread-${index}`,
      title: `Unread discussion ${index}`,
      non_system_message_count: 2,
      message_count: 2,
      updated_at: `2026-07-${String(1 + index).padStart(2, '0')}T10:00:00Z`,
    }));

    render(<DiscussionSidebar {...baseProps} discussions={discussions} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const followUp = document.querySelector('.disc-sidebar-follow-up') as HTMLElement;
    expect(within(followUp).getAllByRole('button', { name: /Unread discussion/ })).toHaveLength(5);
    fireEvent.click(within(followUp).getByText(/15 disc\.showMore/));
    expect(within(followUp).getAllByRole('button', { name: /Unread discussion/ })).toHaveLength(20);
  });
});

describe('DiscussionSidebar — archive paging', () => {
  it('mounts archived discussions 50 at a time', async () => {
    const discussions = Array.from({ length: 55 }, (_, index) => mkDisc({
      id: `archive-${index}`,
      title: `Archived discussion ${index}`,
      archived: true,
      updated_at: `2026-06-${String(1 + (index % 28)).padStart(2, '0')}T10:00:00Z`,
    }));

    render(<DiscussionSidebar {...baseProps} discussions={discussions} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    fireEvent.click(screen.getByText('disc.archived'));
    expect(document.querySelectorAll('.disc-item-title-text')).toHaveLength(50);

    fireEvent.click(screen.getByText('+ 5 disc.showMore'));
    expect(document.querySelectorAll('.disc-item-title-text')).toHaveLength(55);
  });
});

describe('DiscussionSidebar — loose-disc cap (+N more)', () => {
  it('caps loose discs at PROJECT_LOOSE_LIMIT then expands on click', async () => {
    const projects = [mkProject('p1', 'ProjectAlpha')];
    // 12 loose discs > the 10 cap → 2 hidden behind "+2 more".
    const discussions = Array.from({ length: 12 }, (_, i) =>
      mkDisc({
        id: `d${String(i).padStart(2, '0')}`,
        project_id: 'p1',
        title: `Thread ${String(i).padStart(2, '0')}`,
        // Stable desc sort: higher index = newer.
        updated_at: `2026-05-${String(10 + i).padStart(2, '0')}T00:00:00Z`,
      })
    );
    render(<DiscussionSidebar {...baseProps} projects={projects} discussions={discussions} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    // 10 of 12 mounted initially.
    expect(document.querySelectorAll('.disc-item-title-text')).toHaveLength(10);
    const more = screen.getByText(/disc\.showMore/);
    // Hidden count = 2.
    expect(more.textContent).toContain('2');

    fireEvent.click(more);
    // After expand: all 12 mount.
    await waitFor(() => {
      expect(document.querySelectorAll('.disc-item-title-text')).toHaveLength(12);
    });
  });
});

describe('DiscussionSidebar — batch groups', () => {
  const runId = 'run-123';
  const batchDiscs = [
    mkDisc({ id: 'b1', project_id: 'p1', workflow_run_id: runId, title: 'EW-100 — agent A', message_count: 4 }),
    mkDisc({ id: 'b2', project_id: 'p1', workflow_run_id: runId, title: 'EW-100 — agent B', message_count: 4 }),
  ];
  const projects = [mkProject('p1', 'ProjectAlpha')];
  const openBatchMenu = () => {
    fireEvent.click(screen.getByRole('button', { name: 'disc.batchMoreActions' }));
  };

  it('renders a batch folder with a done status pastille', async () => {
    const summaries = [mkBatchSummary({
      run_id: runId,
      quick_prompt_name: 'Compare agents',
      quick_prompt_icon: '🤝',
    })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={summaries}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    // The hierarchy is stable even for one run: QP → run → discussions.
    const campaign = document.querySelector('[data-batch-campaign-key="batch-only::run-123"]');
    const wrap = document.querySelector('[data-batch-key="batch::run-123"]');
    expect(campaign).not.toBeNull();
    expect(wrap).not.toBeNull();
    expect(campaign!.textContent).toContain('Compare agents');
    expect(within(campaign as HTMLElement).getAllByText('Compare agents')).toHaveLength(1);
    expect(wrap!.textContent).not.toContain('Compare agents');
    expect(screen.getByLabelText('disc.batchCampaignStats:1,2')).toBeInTheDocument();
    const runHeader = wrap!.querySelector<HTMLElement>('.disc-batch-header')!;
    expect(runHeader).toHaveAttribute('data-compact', 'true');
    // The campaign tree already owns the indentation. An extra inline margin
    // squeezed the timestamp/status row and made it wrap on narrow sidebars.
    expect(runHeader.style.marginLeft).toBe('');
    // 2/2 messages ≥ 2 and none sending → "✓ 2/2" done pill.
    const pill = wrap!.querySelector('[data-batch-status]');
    expect(pill).not.toBeNull();
    expect(pill!.getAttribute('data-batch-status')).toBe('done');
    expect(pill!.textContent).toContain('2/2');
  });

  it('a running batch (disc in sendingMap) flips the pastille to running', async () => {
    const summaries = [mkBatchSummary({ run_id: runId, quick_prompt_name: 'Compare agents' })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={summaries}
        sendingMap={{ b1: true }}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const wrap = document.querySelector('[data-batch-key="batch::run-123"]')!;
    const pill = wrap.querySelector('[data-batch-status]')!;
    expect(pill.getAttribute('data-batch-status')).toBe('running');
  });

  it('parent-workflow pill navigates via onNavigateWorkflow', async () => {
    const onNavigateWorkflow = vi.fn();
    const summaries = [mkBatchSummary({
      run_id: runId,
      quick_prompt_name: 'Compare agents',
      parent_workflow_id: 'wf-99',
      parent_workflow_name: 'Nightly Audit',
      parent_run_sequence: 5,
    })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={summaries}
        onNavigateWorkflow={onNavigateWorkflow}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    openBatchMenu();
    fireEvent.click(screen.getByRole('button', { name: /Nightly Audit/ }));
    expect(onNavigateWorkflow).toHaveBeenCalledWith('wf-99');
  });

  it('delete-batch button confirms then calls onDeleteBatch with run id + count', async () => {
    const onDeleteBatch = vi.fn();
    // happy-dom has no window.confirm — assign a stub rather than spyOn.
    const confirmStub = vi.fn(() => true);
    window.confirm = confirmStub;
    const summaries = [mkBatchSummary({ run_id: runId, quick_prompt_name: 'Compare agents' })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={summaries}
        onDeleteBatch={onDeleteBatch}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    openBatchMenu();
    fireEvent.click(screen.getByRole('button', { name: 'disc.batchDeleteAction' }));
    expect(confirmStub).toHaveBeenCalled();
    expect(onDeleteBatch).toHaveBeenCalledWith(runId, 2);
  });

  it('retry-batch button (with quick_prompt_id) confirms then calls onRetryBatch', async () => {
    const onRetryBatch = vi.fn();
    const confirmStub = vi.fn(() => true);
    window.confirm = confirmStub;
    const summaries = [mkBatchSummary({
      run_id: runId,
      quick_prompt_id: 'qp-7',
      quick_prompt_name: 'Compare agents',
    })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={summaries}
        onRetryBatch={onRetryBatch}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    openBatchMenu();
    fireEvent.click(screen.getByRole('button', { name: /disc\.batchRetryAction/ }));
    expect(confirmStub).toHaveBeenCalled();
    expect(onRetryBatch).toHaveBeenCalledWith(runId, 'qp-7', ['b1', 'b2']);
  });

  it('review-batch button calls onReviewBatch with run id, label, and child ids', async () => {
    const onReviewBatch = vi.fn();
    const summaries = [mkBatchSummary({
      run_id: runId,
      quick_prompt_name: 'Analyse tickets',
    })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={summaries}
        onReviewBatch={onReviewBatch}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    openBatchMenu();
    fireEvent.click(screen.getByRole('button', { name: 'disc.batchReviewAction' }));
    expect(onReviewBatch).toHaveBeenCalledWith(runId, 'Analyse tickets', ['b1', 'b2']);
  });

  it('compare-batch button opens the side-by-side workspace for any existing batch', async () => {
    const onCompareBatch = vi.fn();
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={[mkBatchSummary({ run_id: runId, quick_prompt_name: 'Compare agents' })]}
        onCompareBatch={onCompareBatch}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    openBatchMenu();
    fireEvent.click(screen.getByRole('button', { name: 'disc.compare.action' }));
    expect(onCompareBatch).toHaveBeenCalledWith(runId, 'Compare agents', ['b1', 'b2']);
  });

  it('moves focus into the action disclosure and restores it on Escape', async () => {
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={[mkBatchSummary({ run_id: runId, quick_prompt_name: 'Compare agents' })]}
        onReviewBatch={vi.fn()}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const trigger = screen.getByRole('button', { name: 'disc.batchMoreActions' });
    fireEvent.click(trigger);
    const action = screen.getByRole('button', { name: 'disc.batchReviewAction' });
    await waitFor(() => expect(action).toHaveFocus());

    fireEvent.keyDown(window, { key: 'Escape' });
    await waitFor(() => expect(trigger).toHaveFocus());
    expect(screen.queryByRole('group', { name: 'disc.batchMoreActions' })).toBeNull();
  });

  it('cancelling the delete confirm does NOT call onDeleteBatch', async () => {
    const onDeleteBatch = vi.fn();
    window.confirm = vi.fn(() => false);
    const summaries = [mkBatchSummary({ run_id: runId, quick_prompt_name: 'Compare agents' })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={summaries}
        onDeleteBatch={onDeleteBatch}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    openBatchMenu();
    fireEvent.click(screen.getByRole('button', { name: 'disc.batchDeleteAction' }));
    expect(onDeleteBatch).not.toHaveBeenCalled();
  });

  it('toggling a batch folder uses the non-persisted run expansion callback', async () => {
    const onToggleBatchRun = vi.fn();
    const summaries = [mkBatchSummary({ run_id: runId, quick_prompt_name: 'Compare agents' })];
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        batchSummaries={summaries}
        onToggleBatchRun={onToggleBatchRun}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const wrap = document.querySelector('[data-batch-key="batch::run-123"]')!;
    const folderBtn = wrap.querySelector<HTMLButtonElement>('button[data-variant="batch"]')!;
    fireEvent.click(folderBtn);
    expect(onToggleBatchRun).toHaveBeenCalledWith('run-123');
  });

  it('groups repeated runs of one Quick Prompt under a single compact heading', async () => {
    const runTwo = 'run-456';
    const discussions = [
      ...batchDiscs,
      mkDisc({ id: 'b3', project_id: 'p1', workflow_run_id: runTwo, title: 'EW-200 — agent A', message_count: 4 }),
      mkDisc({ id: 'b4', project_id: 'p1', workflow_run_id: runTwo, title: 'EW-200 — agent B', message_count: 4 }),
    ];
    const summaries = [
      mkBatchSummary({
        run_id: runId,
        quick_prompt_id: 'qp-review',
        quick_prompt_name: 'PR Review 3.3 shadow',
        quick_prompt_icon: '🔎',
      }),
      mkBatchSummary({
        run_id: runTwo,
        quick_prompt_id: 'qp-review',
        quick_prompt_name: 'PR Review 3.3 shadow',
        quick_prompt_icon: '🔎',
      }),
    ];

    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={discussions}
        batchSummaries={summaries}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const campaign = document.querySelector('[data-batch-campaign-key="qp-batches::qp-review"]')!;
    expect(campaign).not.toBeNull();
    expect(within(campaign as HTMLElement).getAllByText('PR Review 3.3 shadow')).toHaveLength(1);
    expect(within(campaign as HTMLElement).getByLabelText('disc.batchCampaignStats:2,4')).toBeInTheDocument();
    expect(campaign.textContent).toContain('🔀 2');
    expect(campaign.textContent).toContain('💬 4');
    const runRows = campaign.querySelectorAll('.disc-batch-header[data-compact="true"]');
    expect(runRows).toHaveLength(2);
    for (const row of runRows) expect(row.textContent).not.toContain('PR Review 3.3 shadow');
    expect(campaign.textContent).toContain('disc.batchRunId:run123');
    expect(campaign.textContent).toContain('disc.batchRunId:run456');
    // Individual conversations are lazy-mounted: four historical runs no
    // longer flood the sidebar until the user opens the chosen execution.
    expect(screen.queryByText('EW-100 — agent A')).toBeNull();
    expect(screen.queryByText('EW-200 — agent A')).toBeNull();
  });

  it('auto-opens the active run even when historical batch children default closed', async () => {
    render(
      <DiscussionSidebar
        {...baseProps}
        projects={projects}
        discussions={batchDiscs}
        activeId="b1"
        batchSummaries={[mkBatchSummary({ run_id: runId, quick_prompt_name: 'Compare agents' })]}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    expect(screen.getByText('EW-100 — agent A')).toBeInTheDocument();
    expect(screen.getByText('EW-100 — agent B')).toBeInTheDocument();
  });
});

describe('DiscussionSidebar — empty + archives', () => {
  it('shows the empty placeholder when there are no discussions', async () => {
    render(<DiscussionSidebar {...baseProps} discussions={[]} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());
    expect(screen.getByText('disc.empty')).toBeInTheDocument();
  });

  it('archives section toggles open to reveal archived discs', async () => {
    const discussions = [
      mkDisc({ id: 'live', project_id: null, title: 'Live thread' }),
      mkDisc({ id: 'arch', project_id: null, title: 'Archived thread', archived: true }),
    ];
    render(<DiscussionSidebar {...baseProps} discussions={discussions} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    // Archived header renders with count; the disc is collapsed by default.
    const archHeader = screen.getByText('disc.archived').closest('button')!;
    expect(archHeader).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByText('Archived thread')).toBeNull();

    fireEvent.click(archHeader);
    await waitFor(() => {
      expect(screen.getByText('Archived thread')).toBeInTheDocument();
    });
    expect(archHeader).toHaveAttribute('aria-expanded', 'true');
  });
});

describe('DiscussionSidebar — queued (awaiting_agent) indicator', () => {
  // The hourglass must come from the DB field too, NOT only from the live
  // batch_run_child_queued WS frame: that frame is lost when the page isn't
  // mounted at batch-launch time (or on reload / WS reconnect), and every
  // waiting disc rendered as if it were done.
  it('shows the hourglass from disc.awaiting_agent alone (no WS queuedMap)', async () => {
    const discussions = [
      mkDisc({ id: 'q1', project_id: null, title: 'Waiting child', awaiting_agent: true }),
    ];

    render(<DiscussionSidebar {...baseProps} discussions={discussions} queuedMap={{}} />);
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    const dot = document.querySelector('.disc-item-queued');
    expect(dot).not.toBeNull();
    expect(dot!.textContent).toContain('⏳');
  });

  it('running wins over queued: no hourglass when sendingMap is set', async () => {
    const discussions = [
      mkDisc({ id: 'q2', project_id: null, title: 'Running child', awaiting_agent: true }),
    ];

    render(
      <DiscussionSidebar
        {...baseProps}
        discussions={discussions}
        sendingMap={{ q2: true }}
        queuedMap={{}}
      />
    );
    await waitFor(() => expect(projectsApi.discSources).toHaveBeenCalled());

    expect(document.querySelector('.disc-item-queued')).toBeNull();
  });
});

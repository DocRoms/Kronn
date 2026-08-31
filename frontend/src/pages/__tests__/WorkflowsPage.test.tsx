import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import {
  discussions as discussionsApi,
  mcps as mcpsApi,
  quickApis as quickApisApi,
  quickExecs as quickExecsApi,
  quickPrompts as quickPromptsApi,
} from '../../lib/api';
import { WorkflowsPage } from '../WorkflowsPage';
import type { AgentsConfig, QuickApi, QuickExec, QuickPrompt, Workflow, WorkflowSummary } from '../../types/generated';

const mockWorkflowsApi = vi.hoisted(() => ({
  list: vi.fn().mockResolvedValue([]),
  get: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
  trigger: vi.fn(),
  listRuns: vi.fn().mockResolvedValue([]),
  countRuns: vi.fn().mockResolvedValue(0),
  getRun: vi.fn(),
  deleteRun: vi.fn(),
  deleteAllRuns: vi.fn(),
  cancelRun: vi.fn().mockResolvedValue({ run_cancelled: true, child_discs_cancelled: 0 }),
  triggerStream: vi.fn(),
  importWorkflow: vi.fn(),
}));

// 0.8.2 — WorkflowsPage now uses useWebSocket() to listen for live
// WorkflowRunUpdated events. Stub it to a no-op so the test runtime
// doesn't try to open a real WebSocket inside jsdom.
vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: () => ({ connected: false, connectionState: 'connecting' }),
}));

// Mock API — WorkflowsPage calls workflowsApi.list() and skillsApi.list() on mount
vi.mock('../../lib/api', () => ({
  workflows: mockWorkflowsApi,
  skills: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  profiles: {
    list: vi.fn().mockResolvedValue([]),
    get: vi.fn(),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  directives: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
  },
  discussions: {
    create: vi.fn(),
  },
  quickPrompts: {
    list: vi.fn().mockResolvedValue([]),
    metrics: vi.fn().mockResolvedValue([]),
    history: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    setPinned: vi.fn(),
    delete: vi.fn(),
    batchRun: vi.fn(),
    exportQp: vi.fn(),
    importQp: vi.fn(),
  },
  quickApis: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    setPinned: vi.fn(),
    delete: vi.fn(),
    runQa: vi.fn(),
    exportQa: vi.fn(),
    importQa: vi.fn(),
  },
  quickExecs: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    setPinned: vi.fn(),
    delete: vi.fn(),
    run: vi.fn(),
    export: vi.fn(),
    import: vi.fn(),
  },
  pages: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
  },
  config: {
    getUiLanguage: vi.fn().mockResolvedValue('fr'),
    saveUiLanguage: vi.fn().mockResolvedValue(undefined),
    // 0.8.6 phase 4 — WorkflowWizard reads default tier on mount.
    getServerConfig: vi.fn().mockResolvedValue({ default_model_tier: 'default' }),
  },
  // WorkflowWizard loads the MCP overview at mount (ApiCall plugin picker).
  mcps: {
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], customized_contexts: [], incompatibilities: [] }),
    registry: vi.fn().mockResolvedValue([]),
  },
  // 0.8.10 — WorkflowWizard fetches installed Ollama models at mount for the
  // per-step model picker (datalist on Ollama steps).
  ollama: {
    models: vi.fn().mockResolvedValue({ models: [] }),
    health: vi.fn().mockResolvedValue({ reachable: false, models: [] }),
  },
}));

const defaultModelTiers = {
  claude_code: { economy: null, reasoning: null },
  codex: { economy: null, reasoning: null },
  gemini_cli: { economy: null, reasoning: null },
  kiro: { economy: null, reasoning: null },
  vibe: { economy: null, reasoning: null },
  copilot_cli: { economy: null, reasoning: null },
  ollama: { economy: null, reasoning: null },
  lite_llm: { economy: null, reasoning: null },
  nvidia: { economy: null, reasoning: null },
};

const restrictedConfig: AgentsConfig = {
  claude_code: { path: null, installed: true, version: null, full_access: false },
  codex: { path: null, installed: true, version: null, full_access: false },
  gemini_cli: { path: null, installed: true, version: null, full_access: false },
  kiro: { path: null, installed: false, version: null, full_access: false },
  vibe: { path: null, installed: false, version: null, full_access: false },
  copilot_cli: { path: null, installed: false, version: null, full_access: false },
  ollama: { path: null, installed: false, version: null, full_access: false },
  lite_llm: { path: null, installed: false, version: null, full_access: false },
  nvidia: { path: null, installed: false, version: null, full_access: false },
  model_tiers: defaultModelTiers,
};

const fullConfig: AgentsConfig = {
  claude_code: { path: null, installed: true, version: null, full_access: true },
  codex: { path: null, installed: true, version: null, full_access: true },
  gemini_cli: { path: null, installed: true, version: null, full_access: true },
  kiro: { path: null, installed: false, version: null, full_access: true },
  vibe: { path: null, installed: false, version: null, full_access: true },
  copilot_cli: { path: null, installed: false, version: null, full_access: false },
  ollama: { path: null, installed: false, version: null, full_access: false },
  lite_llm: { path: null, installed: false, version: null, full_access: false },
  nvidia: { path: null, installed: false, version: null, full_access: false },
  model_tiers: defaultModelTiers,
};

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  sessionStorage.removeItem('kronn:postQpImproved');
  localStorage.removeItem('kronn:automationNavigation');
  localStorage.removeItem('kronn:automationCollapsedSections');
});

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
};

const openAutomationActions = () => {
  fireEvent.click(screen.getByRole('button', { name: 'Créer ou importer' }));
  return screen.getByRole('dialog', { name: 'Créer ou importer' });
};

const chooseAutomationAction = (name: string) => {
  const dialog = openAutomationActions();
  fireEvent.click(within(dialog).getByRole('button', { name }));
};

describe('WorkflowsPage', () => {
  it('renders the automation empty state through the real page adapter', async () => {
    await wrap(<WorkflowsPage projects={[]} />);
    expect(screen.getByText('Aucun workflow configuré')).toBeInTheDocument();
  });

  it('covers the narrow rail, empty list, selection, and open menu on the real Automations page', async () => {
    vi.stubGlobal('matchMedia', vi.fn().mockImplementation((query: string) => ({ matches: true, media: query, addEventListener: vi.fn(), removeEventListener: vi.fn() })));
    await wrap(<WorkflowsPage projects={[]} />);
    expect(screen.getByText('Aucun workflow configuré')).toBeInTheDocument();

    const workflow = {
      id: 'wf-narrow', name: 'Narrow report', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 0, misconfigured_step_count: 0,
      enabled: true, pinned: false, last_run: null, created_at: '2026-01-01T00:00:00Z',
    } as WorkflowSummary;
    cleanup();
    mockWorkflowsApi.list.mockResolvedValueOnce([workflow]);
    await wrap(<WorkflowsPage projects={[]} />);
    const sidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    expect(within(sidebar).getByRole('button', { name: 'Ouvrir Narrow report' })).toBeInTheDocument();
    fireEvent.click(within(sidebar).getByRole('button', { name: 'Ouvrir Narrow report' }));
    expect(document.querySelector('.automation-page')).toHaveAttribute('data-has-selection', 'true');
    expect(openAutomationActions()).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Fermer la liste' }));
    expect(screen.getByRole('button', { name: 'Ouvrir la liste' })).toHaveClass('collection-shell-sidebar-rail');
  });

  it('collapses to the shared Discussions-style rail and reopens the sidebar', async () => {
    await wrap(<WorkflowsPage projects={[]} />);
    const collapse = screen.getByRole('button', { name: 'Fermer la liste' });
    expect(collapse).toHaveClass('collection-shell-collapse-button');
    fireEvent.click(collapse);
    expect(screen.queryByRole('complementary', { name: 'Automatisation' })).toBeNull();
    const rail = screen.getByRole('button', { name: 'Ouvrir la liste' });
    expect(rail).toHaveClass('collection-shell-sidebar-rail');
    fireEvent.click(rail);
    expect(screen.getByRole('complementary', { name: 'Automatisation' })).toBeInTheDocument();
  });

  it('groups creation and import paths behind one green sidebar action', async () => {
    vi.mocked(mcpsApi.overview).mockResolvedValueOnce({
      servers: [{ id: 'api-server', name: 'Configured API', api_spec: {} }],
      configs: [{ id: 'api-config', server_id: 'api-server' }],
      customized_contexts: [],
      incompatibilities: [],
    } as never);

    await wrap(<WorkflowsPage projects={[]} onNavigateDiscussion={vi.fn()} />);

    const primaryAction = screen.getByRole('button', { name: 'Créer ou importer' });
    expect(primaryAction).toHaveClass('collection-shell-primary-action', 'disc-sidebar-new-btn');
    expect(screen.queryByRole('button', { name: 'Importer' })).toBeNull();

    const dialog = openAutomationActions();
    expect(await within(dialog).findByRole('button', { name: 'Nouveau Quick API' })).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: "Créer avec l'IA" })).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Nouveau workflow' })).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Nouveau prompt' })).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Nouveau Quick Exec' })).toBeInTheDocument();
    expect(within(dialog).getByRole('button', { name: 'Importer' })).toBeInTheDocument();

    fireEvent.keyDown(window, { key: 'Escape' });
    expect(screen.queryByRole('dialog', { name: 'Créer ou importer' })).toBeNull();

    openAutomationActions();
    fireEvent.click(document.querySelector('.wf-import-modal-backdrop') as HTMLElement);
    expect(screen.queryByRole('dialog', { name: 'Créer ou importer' })).toBeNull();

    chooseAutomationAction('Nouveau prompt');
    expect(screen.getByRole('heading', { name: 'Nouveau prompt' })).toBeInTheDocument();
  });

  it('uses one searchable sidebar ordered Workflow → API → Prompt → Exec', async () => {
    const alpha = {
      id: 'wf-alpha', name: 'Alpha report', project_id: 'p-alpha', project_name: 'Alpha',
      trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0,
      enabled: true, pinned: false, last_run: null, created_at: '2026-01-01T00:00:00Z',
    } as WorkflowSummary;
    const beta = { ...alpha, id: 'wf-beta', name: 'Beta report', project_id: null, project_name: null };
    mockWorkflowsApi.list.mockResolvedValueOnce([alpha, beta]);
    mockWorkflowsApi.get.mockResolvedValueOnce({
      id: alpha.id,
      name: alpha.name,
      project_id: alpha.project_id,
      trigger: { type: 'Manual' },
      steps: [], actions: [], safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
      workspace_config: null, concurrency_limit: null, enabled: true, pinned: false,
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    } as Workflow);
    mockWorkflowsApi.listRuns.mockResolvedValueOnce([]);

    await wrap(<WorkflowsPage projects={[{ id: 'p-alpha', name: 'Alpha' } as never]} />);

    const sidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    const kinds = within(sidebar).getAllByRole('button').filter(button => button.dataset.tourId?.startsWith('automation-kind-'));
    expect(kinds.map(button => button.dataset.tourId)).toEqual([
      'automation-kind-workflow',
      'automation-kind-quick-api',
      'automation-kind-quick-prompt',
      'automation-kind-quick-exec',
    ]);

    fireEvent.change(within(sidebar).getByRole('textbox', { name: 'Rechercher une automatisation…' }), {
      target: { value: 'Alpha' },
    });
    expect(within(sidebar).getByRole('button', { name: 'Ouvrir Alpha report' })).toBeInTheDocument();
    expect(within(sidebar).queryByRole('button', { name: 'Ouvrir Beta report' })).toBeNull();

    fireEvent.keyDown(window, { key: '/' });
    expect(within(sidebar).getByRole('textbox', { name: 'Rechercher une automatisation…' })).toHaveFocus();

    const projectFilterButton = within(sidebar).getByRole('button', { name: 'Filtrer les automatisations par projet' });
    expect(projectFilterButton).toHaveClass('collection-shell-search-action-icon');
    expect(projectFilterButton.querySelector('span')).toBeNull();
    expect(projectFilterButton).toHaveAttribute('aria-expanded', 'false');
    expect(projectFilterButton).not.toHaveAttribute('aria-controls');
    expect(within(sidebar).queryByRole('combobox', { name: 'Filtrer les automatisations par projet' })).toBeNull();

    fireEvent.click(projectFilterButton);
    expect(projectFilterButton).toHaveAttribute('aria-expanded', 'true');
    expect(projectFilterButton).toHaveAttribute('aria-controls', 'automation-project-filter-options');
    const projectFilter = within(sidebar).getByRole('combobox', { name: 'Filtrer les automatisations par projet' });
    expect(projectFilter.closest('.collection-shell-search-options')).toHaveAttribute(
      'id',
      'automation-project-filter-options',
    );
    fireEvent.change(projectFilter, {
      target: { value: 'p-alpha' },
    });
    expect(projectFilterButton).toHaveAttribute('data-active', 'true');
    expect(projectFilterButton.querySelector('strong')).toBeNull();

    fireEvent.keyDown(projectFilter, { key: 'Escape' });
    expect(projectFilterButton).toHaveAttribute('aria-expanded', 'false');
    expect(projectFilterButton).toHaveFocus();
    expect(within(sidebar).queryByRole('combobox', { name: 'Filtrer les automatisations par projet' })).toBeNull();

    fireEvent.click(within(sidebar).getByRole('button', { name: 'Effacer la recherche' }));
    expect(within(sidebar).getByRole('button', { name: 'Ouvrir Alpha report' })).toBeInTheDocument();
    expect(within(sidebar).queryByRole('button', { name: 'Ouvrir Beta report' })).toBeNull();

    await act(async () => {
      fireEvent.click(within(sidebar).getByRole('button', { name: 'Ouvrir Alpha report' }));
    });
    await waitFor(() => expect(mockWorkflowsApi.get).toHaveBeenCalledWith('wf-alpha'));
    const detailPane = screen.getByTestId('workflow-detail-pane');
    expect(detailPane).toBeInTheDocument();
    expect(detailPane.querySelector('.wf-detail-header')).toHaveClass('collection-detail-header');
    expect(document.querySelector('.automation-page-header')).toBeNull();
  });

  it('keeps arrow-key navigation on grouped Automation rows rendered by CollectionShell', async () => {
    const alpha = {
      id: 'wf-alpha', name: 'Alpha report', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0,
      enabled: true, pinned: false, last_run: null, created_at: '2026-01-01T00:00:00Z',
    } as WorkflowSummary;
    const beta = { ...alpha, id: 'wf-beta', name: 'Beta report' };
    mockWorkflowsApi.list.mockResolvedValueOnce([alpha, beta]);

    await wrap(<WorkflowsPage projects={[]} />);

    const sidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    const alphaButton = within(sidebar).getByRole('button', { name: 'Ouvrir Alpha report' });
    const betaButton = within(sidebar).getByRole('button', { name: 'Ouvrir Beta report' });
    alphaButton.focus();
    fireEvent.keyDown(alphaButton, { key: 'ArrowDown' });
    expect(betaButton).toHaveFocus();
  });

  it('opens a Quick Exec from the shared sidebar in the common detail area', async () => {
    const quickExec = {
      id: 'qe-aws', name: 'CloudWatch errors', icon: '⌘', description: 'Collecte les erreurs',
      pinned: false,
      project_id: null, command: 'aws',
      args: ['logs', 'start-query', '--log-group-name', '/aws/caddy/production', '--query-string', 'fields @timestamp, @message | filter status >= 500 | sort @timestamp desc'],
      timeout_secs: 60,
      output_format: 'json', variables: [],
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    } as QuickExec;
    vi.mocked(quickExecsApi.list).mockResolvedValueOnce([quickExec]);

    await wrap(<WorkflowsPage projects={[]} />);
    const sidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    expect(sidebar.querySelector('[data-tour-id="automation-kind-quick-exec"]'))
      .toHaveTextContent('Quick Execs (CLI)');
    await act(async () => {
      fireEvent.click(within(sidebar).getByRole('button', { name: 'Ouvrir CloudWatch errors' }));
    });

    expect(screen.getByRole('heading', { name: 'CloudWatch errors' })).toBeInTheDocument();
    const card = document.querySelector('.automation-viewer .qe-card');
    expect(card).not.toBeNull();
    expect(card).toHaveAttribute('data-detail', 'true');
    expect(card).toHaveTextContent('aws');
    expect(card).toHaveTextContent('JSON');
    const commandPreview = card?.querySelector('.qe-command-preview');
    expect(commandPreview?.querySelectorAll('code')).toHaveLength(1);
    expect(commandPreview?.querySelector('.qe-command-line')).toHaveAttribute(
      'title',
      expect.stringContaining('filter status >= 500'),
    );
    expect(within(card as HTMLElement).getByRole('button', { name: 'Tester' }))
      .toHaveClass('qp-launch-btn');
    expect(screen.getByRole('region', { name: 'Éditeur Quick Exec' })).toBeInTheDocument();
    expect(screen.getByDisplayValue('CloudWatch errors')).toBeInTheDocument();
  });

  it('restores the selected automation and collapsed sidebar sections', async () => {
    const quickExec = {
      id: 'qe-persisted', name: 'Persistent CLI', icon: '⌘', description: 'Persists navigation',
      pinned: false,
      project_id: null, command: 'aws', args: ['sts', 'get-caller-identity'], timeout_secs: 30,
      output_format: 'json', variables: [],
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    } as QuickExec;
    vi.mocked(quickExecsApi.list)
      .mockResolvedValueOnce([quickExec])
      .mockResolvedValueOnce([quickExec]);

    const first = await wrap(<WorkflowsPage projects={[]} />);
    const firstSidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    const workflowSection = firstSidebar.querySelector('[data-tour-id="automation-kind-workflow"]') as HTMLButtonElement;
    fireEvent.click(workflowSection);
    expect(workflowSection).toHaveAttribute('aria-expanded', 'false');

    const search = within(firstSidebar).getByRole('textbox', { name: 'Rechercher une automatisation…' });
    fireEvent.change(search, { target: { value: 'Persistent' } });
    expect(workflowSection).toHaveAttribute('aria-expanded', 'true');
    fireEvent.change(search, { target: { value: '' } });
    expect(workflowSection).toHaveAttribute('aria-expanded', 'false');

    fireEvent.click(within(firstSidebar).getByRole('button', { name: 'Ouvrir Persistent CLI' }));
    expect(await screen.findByRole('heading', { name: 'Persistent CLI' })).toBeInTheDocument();
    first.unmount();

    await wrap(<WorkflowsPage projects={[]} />);
    const restoredSidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    expect(restoredSidebar.querySelector('[data-tour-id="automation-kind-workflow"]'))
      .toHaveAttribute('aria-expanded', 'false');
    expect(await screen.findByRole('heading', { name: 'Persistent CLI' })).toBeInTheDocument();
  });

  it('drops a persisted automation selection when its resource no longer exists', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({
      tab: 'quickExecs',
      resourceId: 'qe-deleted',
    }));

    await wrap(<WorkflowsPage projects={[]} />);

    await waitFor(() => {
      expect(JSON.parse(localStorage.getItem('kronn:automationNavigation') ?? '{}'))
        .toEqual({ tab: 'quickExecs', resourceId: null });
    });
    expect(document.querySelector('.automation-page')).toHaveAttribute('data-has-selection', 'false');
  });

  it('reloads the persisted workflow detail instead of only highlighting its row', async () => {
    const summary = {
      id: 'wf-persisted', name: 'Persisted workflow', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 0, misconfigured_step_count: 0,
      enabled: true, pinned: false, last_run: null, created_at: '2026-01-01T00:00:00Z',
    } as WorkflowSummary;
    const workflow = {
      id: summary.id, name: summary.name, project_id: null,
      trigger: { type: 'Manual' }, steps: [], actions: [],
      safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
      workspace_config: null, concurrency_limit: null, enabled: true, pinned: false,
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    } as Workflow;
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({
      tab: 'workflows',
      resourceId: summary.id,
    }));
    mockWorkflowsApi.list.mockResolvedValueOnce([summary]);
    mockWorkflowsApi.get.mockResolvedValueOnce(workflow);

    await wrap(<WorkflowsPage projects={[]} />);

    await waitFor(() => expect(mockWorkflowsApi.get).toHaveBeenCalledWith(summary.id));
    expect(screen.getByTestId('workflow-detail-pane')).toBeInTheDocument();
  });

  it('opens a Quick Prompt editor directly while keeping its command actions', async () => {
    const quickPrompt: QuickPrompt = {
      id: 'qp-summary', name: 'Summarize release', icon: '✍️', description: 'Résumé de livraison',
      pinned: false,
      prompt_template: 'Résume {{changes}}', project_id: null, agent: 'Codex', tier: 'default',
      variables: [{ name: 'changes', label: 'Changements', placeholder: '', description: null, required: true, source: 'user_input', source_ref: null, allow_manual_override: false }],
      skill_ids: [], profile_ids: [], directive_ids: [],
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    };
    vi.mocked(quickPromptsApi.list).mockResolvedValueOnce([quickPrompt]);
    vi.mocked(quickPromptsApi.update).mockClear();
    vi.mocked(quickPromptsApi.update).mockResolvedValueOnce(quickPrompt);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['Codex']} agentAccess={fullConfig} />);
    const sidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    await act(async () => {
      fireEvent.click(within(sidebar).getByRole('button', { name: 'Ouvrir Summarize release' }));
    });

    const commandBar = document.querySelector('.automation-viewer .qp-card[data-detail="true"]');
    expect(commandBar).not.toBeNull();
    expect(within(commandBar as HTMLElement).getByRole('button', { name: /Comparer/ })).toBeInTheDocument();
    expect(screen.getByDisplayValue('Summarize release')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Modifier Summarize release' })).toBeNull();

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Enregistrer' }));
    });
    await waitFor(() => expect(quickPromptsApi.update).toHaveBeenCalledWith(
      'qp-summary',
      expect.objectContaining({ name: 'Summarize release' }),
    ));
  });

  it('opens the Quick Prompts tab from the one-shot deploy target without an effect redirect', async () => {
    sessionStorage.setItem('kronn:postQpImproved', 'qp-deployed');

    await wrap(<WorkflowsPage projects={[]} />);

    expect(screen.getByRole('button', { name: /Quick Prompts/ })).toHaveAttribute('data-active', 'true');
    expect(sessionStorage.getItem('kronn:postQpImproved')).toBeNull();
  });

  it('renders with various agentAccess configs and shows create button', async () => {
    // Without agentAccess
    const { unmount: u1 } = await wrap(<WorkflowsPage projects={[]} />);
    expect(screen.getByText('Automatisation')).toBeDefined();
    expect(screen.getByRole('button', { name: 'Créer ou importer' })).toBeDefined();
    u1();

    // With restricted agentAccess
    const { unmount: u2 } = await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode', 'Codex']}
        agentAccess={restrictedConfig}
      />
    );
    expect(screen.getByText('Automatisation')).toBeDefined();
    expect(screen.getByRole('button', { name: 'Créer ou importer' })).toBeDefined();
    u2();

    // With full access agentAccess
    await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode']}
        agentAccess={fullConfig}
      />
    );
    expect(screen.getByText('Automatisation')).toBeDefined();
    expect(screen.getByRole('button', { name: 'Créer ou importer' })).toBeDefined();
  });

  // ─── Mobile responsive ─────────────────────────────────────────────────

  it('renders layout without error on mobile viewport', async () => {
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: query.includes('767'),
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Page title and create button should still render on mobile
    expect(screen.getByText('Automatisation')).toBeDefined();
    expect(screen.getByRole('button', { name: 'Créer ou importer' })).toBeDefined();

    // The layout should use column direction on mobile (flex-direction: column)
    // Just verify no crash and content is accessible
    const body = document.body.textContent!;
    expect(body).toContain('Workflows');

    // Restore default matchMedia
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    });
  });

  // ─── Workflow edit preserves existing steps ───────────────────────────────

  it('populates steps when editing an existing workflow', async () => {
    const sampleWorkflow: Workflow = {
      id: 'wf-1',
      name: 'My Workflow',
      project_id: null,
      trigger: { type: 'Manual' },
      steps: [
        { name: 'analyze', agent: 'ClaudeCode', prompt_template: 'Analyse this bug', mode: { type: 'Normal' }, step_type: { type: 'Agent' }, output_format: { type: 'FreeText' } },
        { name: 'fix', agent: 'Codex', prompt_template: 'Fix: {{previous_step.output}}', mode: { type: 'Normal' }, step_type: { type: 'Agent' }, output_format: { type: 'FreeText' } },
      ],
      actions: [],
      safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
      workspace_config: null,
      concurrency_limit: null,
      enabled: true,
      pinned: false,
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    };

    const summaries: WorkflowSummary[] = [{
      id: 'wf-1',
      name: 'My Workflow',
      project_id: null,
      project_name: null,
      trigger_type: 'manual',
      step_count: 2,
      misconfigured_step_count: 0,
      enabled: true,
      pinned: false,
      last_run: null,
      created_at: '2026-01-01T00:00:00Z',
    }];

    mockWorkflowsApi.list.mockResolvedValue(summaries);
    mockWorkflowsApi.get.mockResolvedValue(sampleWorkflow);
    mockWorkflowsApi.listRuns.mockResolvedValue([]);

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode', 'Codex']} agentAccess={fullConfig} />
    );

    // Click on the workflow in the list to open detail
    const workflowCard = screen.getByRole('button', { name: 'Ouvrir My Workflow' });
    await act(async () => { fireEvent.click(workflowCard); });

    // Wait for the detail to load and click "Edit"
    await waitFor(() => {
      expect(screen.getByText('Éditer')).toBeDefined();
    });

    await act(async () => {
      fireEvent.click(screen.getByText('Éditer'));
    });

    // Navigate to wizard step 2 (Steps) — click "Suivant" twice (step 0 → 1 → 2)
    const nextButtons = screen.getAllByText('Suivant');
    await act(async () => { fireEvent.click(nextButtons[nextButtons.length - 1]); });
    const nextButtons2 = screen.getAllByText('Suivant');
    await act(async () => { fireEvent.click(nextButtons2[nextButtons2.length - 1]); });

    // Verify both steps are present with their names and prompts
    expect(screen.getByDisplayValue('analyze')).toBeDefined();
    expect(screen.getByDisplayValue('fix')).toBeDefined();
    expect(screen.getByDisplayValue('Analyse this bug')).toBeDefined();
    expect(screen.getByDisplayValue('Fix: {{previous_step.output}}')).toBeDefined();
  });

  it('pinned workflows surface in a cross-project Favoris group (and stay in their project group)', async () => {
    const base = {
      project_id: 'p1', project_name: 'Proj', trigger_type: 'manual',
      step_count: 1, misconfigured_step_count: 0, enabled: true,
      last_run: null, created_at: '2026-01-01T00:00:00Z',
    };
    mockWorkflowsApi.list.mockResolvedValue([
      { ...base, id: 'wf-pin', name: 'Pinned WF', pinned: true },
      { ...base, id: 'wf-reg', name: 'Regular WF', pinned: false },
    ]);

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // The pinned workflow appears in the unified sidebar's Favorites +
    // Workflows sections and in the legacy card list's Favorites + project
    // groups. The ordinary workflow appears once in each regular list.
    expect(screen.getAllByText('Favoris')).toHaveLength(2);
    expect(screen.getAllByText('Pinned WF')).toHaveLength(4);
    expect(screen.getAllByText('Regular WF')).toHaveLength(2);
  });

  it('keeps workflow favorites rendered in the unified sidebar while a detail is selected', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({
      tab: 'workflows',
      resourceId: 'wf-pin',
    }));
    const summary: WorkflowSummary = {
      id: 'wf-pin', name: 'Pinned detail', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0,
      enabled: true, pinned: true, last_run: null, created_at: '2026-01-01T00:00:00Z',
    };
    mockWorkflowsApi.list.mockResolvedValueOnce([summary]);
    mockWorkflowsApi.get.mockResolvedValueOnce({
      id: summary.id, name: summary.name, project_id: null,
      trigger: { type: 'Manual' }, steps: [], actions: [],
      safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
      workspace_config: null, concurrency_limit: null, enabled: true, pinned: true,
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    } as Workflow);

    await wrap(<WorkflowsPage projects={[]} />);

    const sidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    expect(within(sidebar).getByRole('button', { name: 'Favoris 1' })).toHaveClass('collection-favorites-header');
    expect(within(sidebar).getAllByRole('button', { name: 'Ouvrir Pinned detail' })).toHaveLength(2);
    expect(screen.getByTestId('workflow-detail-pane')).toBeInTheDocument();
  });

  it('groups pinned Quick API, Prompt and Exec resources and toggles them with one API shape', async () => {
    const quickPrompt: QuickPrompt = {
      id: 'qp-favorite', name: 'Prompt favori', icon: '✍️', prompt_template: 'Résume',
      variables: [], agent: 'Codex', project_id: null, skill_ids: [], profile_ids: [],
      directive_ids: [], tier: 'default', description: '', pinned: true,
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    };
    const quickApi: QuickApi = {
      id: 'qa-favorite', name: 'API favorite', icon: '🔌', description: '', project_id: null,
      api_plugin_slug: 'demo', api_config_id: 'cfg', api_endpoint_path: '/items',
      variables: [], profile_ids: [], directive_ids: [], pinned: true,
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    };
    const quickExec: QuickExec = {
      id: 'qe-favorite', name: 'Exec favori', icon: '⌘', description: '', project_id: null,
      command: 'git', args: ['status'], timeout_secs: 30, output_format: 'text',
      variables: [], pinned: true,
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    };
    vi.mocked(quickPromptsApi.list).mockResolvedValueOnce([quickPrompt]);
    vi.mocked(quickApisApi.list).mockResolvedValueOnce([quickApi]);
    vi.mocked(quickExecsApi.list).mockResolvedValueOnce([quickExec]);
    mockWorkflowsApi.list.mockResolvedValueOnce([]);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['Codex']} agentAccess={fullConfig} />);

    const sidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    const favorites = sidebar.querySelector('.disc-sidebar-favorites') as HTMLElement;
    expect(within(favorites).getByRole('button', { name: 'Favoris 3' })).toHaveClass('collection-favorites-header');
    expect(within(favorites).getByRole('button', { name: 'Ouvrir API favorite' })).toBeInTheDocument();
    expect(within(favorites).getByRole('button', { name: 'Ouvrir Prompt favori' })).toBeInTheDocument();
    expect(within(favorites).getByRole('button', { name: 'Ouvrir Exec favori' })).toBeInTheDocument();

    fireEvent.click(within(favorites).getByRole('button', {
      name: 'Retirer des favoris · Prompt favori',
    }));
    await waitFor(() => expect(quickPromptsApi.setPinned).toHaveBeenCalledWith('qp-favorite', false));
  });

  it('the star toggle pins a workflow through the partial update', async () => {
    mockWorkflowsApi.list.mockResolvedValue([{
      id: 'wf-reg', name: 'Regular WF', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0,
      enabled: true, pinned: false, last_run: null, created_at: '2026-01-01T00:00:00Z',
    }]);
    mockWorkflowsApi.update.mockResolvedValue({});

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    await act(async () => {
      fireEvent.click(screen.getByLabelText('Ajouter aux favoris'));
    });
    expect(mockWorkflowsApi.update).toHaveBeenCalledWith('wf-reg', { pinned: true });
  });

  it('uses the shared row menu and complete keyboard footer', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
    mockWorkflowsApi.list.mockResolvedValue([{
      id: 'wf-reg', name: 'Regular WF', project_id: null, project_name: null,
      trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0,
      enabled: true, pinned: false, last_run: null, created_at: '2026-01-01T00:00:00Z',
    }]);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);
    const sidebar = screen.getByRole('complementary', { name: 'Automatisation' });
    fireEvent.click(within(sidebar).getByRole('button', { name: 'Plus d’actions · Regular WF' }));
    fireEvent.click(within(sidebar).getByRole('menuitem', { name: 'Copier l’ID' }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('wf-reg'));

    const footer = sidebar.querySelector('.disc-sidebar-footer') as HTMLElement;
    expect(footer).toHaveTextContent('Sélectionne une ressource pour l’ouvrir');
    expect(within(footer).getByText('↑↓')).toBeInTheDocument();
    expect(within(footer).getByText('/')).toBeInTheDocument();
  });

  it('shows a "needs config" badge on the card when misconfigured_step_count > 0', async () => {
    // A freshly AI-generated workflow with an unwired API step: the backend
    // reports misconfigured_step_count > 0 and the card must surface it so the
    // user knows there's wiring left before the workflow can run.
    mockWorkflowsApi.list.mockResolvedValue([{
      id: 'wf-bad',
      name: 'Ticket → PR',
      project_id: null,
      project_name: null,
      trigger_type: 'manual',
      step_count: 4,
      misconfigured_step_count: 3,
      enabled: true,
      last_run: null,
      created_at: '2026-01-01T00:00:00Z',
    }]);

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    await waitFor(() => expect(screen.getAllByText('Ticket → PR')).toHaveLength(2));
    // i18n: 'wf.needsConfig' = '{0} à configurer' → "3 à configurer"
    expect(screen.getByText('3 à configurer')).toBeDefined();
  });

  it('hides the "needs config" badge when misconfigured_step_count is 0', async () => {
    mockWorkflowsApi.list.mockResolvedValue([{
      id: 'wf-ok',
      name: 'Clean WF',
      project_id: null,
      project_name: null,
      trigger_type: 'manual',
      step_count: 2,
      misconfigured_step_count: 0,
      enabled: true,
      last_run: null,
      created_at: '2026-01-01T00:00:00Z',
    }]);

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    await waitFor(() => expect(screen.getAllByText('Clean WF')).toHaveLength(2));
    expect(screen.queryByText(/à configurer/)).toBeNull();
  });

  // ─── Wizard validation errors on summary page ───────────────────────────

  it('shows validation error for missing prompt on summary step (simple mode)', async () => {
    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Click "Nouveau workflow" to open wizard
    chooseAutomationAction('Nouveau workflow');

    // Wizard starts in simple mode (3 steps: infos → task → summary)
    // Fill workflow name on step 0
    const nameInput = screen.getByPlaceholderText('ex: Auto-fix 5xx errors');
    await act(async () => { fireEvent.change(nameInput, { target: { value: 'Test WF' } }); });

    // Navigate to summary step: click "Suivant" 2 times (0→1→2)
    for (let i = 0; i < 2; i++) {
      const nextBtns = screen.getAllByText(/Suivant/);
      await act(async () => { fireEvent.click(nextBtns[nextBtns.length - 1]); });
    }

    // Should show validation error for missing prompt (step has empty prompt_template)
    await waitFor(() => {
      expect(document.body.textContent).toContain('Prompt manquant');
    });
  });

  it('disables next button when workflow name is empty on step 0', async () => {
    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Click "Nouveau workflow" to open wizard
    chooseAutomationAction('Nouveau workflow');

    // The "Suivant" button should be disabled since name is empty
    const nextBtns = screen.getAllByText(/Suivant/);
    const nextBtn = nextBtns[nextBtns.length - 1];
    expect(nextBtn.closest('button')!.disabled).toBe(true);
  });

  it('creates a workflow through the wizard in advanced mode', async () => {
    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Click "Nouveau workflow" to open wizard
    chooseAutomationAction('Nouveau workflow');

    // Switch to advanced mode (wizard starts in simple mode)
    const advBtn = screen.getByText(/Avancé/);
    await act(async () => { fireEvent.click(advBtn); });

    // Step 0: fill the workflow name
    const nameInput = screen.getByPlaceholderText('ex: Auto-fix 5xx errors');
    await act(async () => { fireEvent.change(nameInput, { target: { value: 'My CI Workflow' } }); });

    // The "Suivant" button should now be enabled
    let nextBtns = screen.getAllByText(/Suivant/);
    let nextBtn = nextBtns[nextBtns.length - 1];
    expect(nextBtn.closest('button')!.disabled).toBe(false);

    // Navigate to step 1 (trigger)
    await act(async () => { fireEvent.click(nextBtn); });

    // Step 1 should show trigger options — verify the "Manually" trigger button is visible
    expect(document.body.textContent).toContain('Manuellement');

    // Navigate to step 2 (steps)
    nextBtns = screen.getAllByText(/Suivant/);
    nextBtn = nextBtns[nextBtns.length - 1];
    await act(async () => { fireEvent.click(nextBtn); });

    // Step 2 should show step configuration — verify the step name input exists
    const stepNameInputs = document.querySelectorAll('input[placeholder]');
    expect(stepNameInputs.length).toBeGreaterThan(0);

    // Navigate to step 3 (config)
    nextBtns = screen.getAllByText(/Suivant/);
    nextBtn = nextBtns[nextBtns.length - 1];
    await act(async () => { fireEvent.click(nextBtn); });

    // Navigate to step 4 (summary)
    nextBtns = screen.getAllByText(/Suivant/);
    nextBtn = nextBtns[nextBtns.length - 1];
    await act(async () => { fireEvent.click(nextBtn); });

    // Summary step should show the workflow name we entered
    expect(document.body.textContent).toContain('My CI Workflow');
  });

  it('creates a workflow in simple mode (3 steps)', async () => {
    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Click "Nouveau workflow" to open wizard
    chooseAutomationAction('Nouveau workflow');

    // Wizard starts in simple mode — should show "Simple" and "Avancé" toggles
    expect(screen.getByText(/Simple/)).toBeDefined();
    expect(screen.getByText(/Avancé/)).toBeDefined();

    // Step 0: fill the workflow name
    const nameInput = screen.getByPlaceholderText('ex: Auto-fix 5xx errors');
    await act(async () => { fireEvent.change(nameInput, { target: { value: 'Quick Task' } }); });

    // Navigate to step 1 (task)
    let nextBtns = screen.getAllByText(/Suivant/);
    await act(async () => { fireEvent.click(nextBtns[nextBtns.length - 1]); });

    // Step 1 (simple task): should show agent selector, prompt, and trigger toggle
    expect(document.body.textContent).toContain('Agent');
    expect(document.body.textContent).toContain('Manuellement');
    expect(document.body.textContent).toContain('Sur un planning');

    // Fill the prompt
    const promptInput = screen.getByPlaceholderText(/Décrivez la tâche/);
    await act(async () => { fireEvent.change(promptInput, { target: { value: 'Analyse ce projet' } }); });

    // Switch to scheduled trigger
    const scheduleBtn = screen.getByText(/Sur un planning/);
    await act(async () => { fireEvent.click(scheduleBtn); });

    // Should show frequency picker (the "Tous les" label)
    expect(document.body.textContent).toContain('Tous les');

    // Navigate to step 2 (summary)
    nextBtns = screen.getAllByText(/Suivant/);
    await act(async () => { fireEvent.click(nextBtns[nextBtns.length - 1]); });

    // Summary should show the workflow name and cron info
    expect(document.body.textContent).toContain('Quick Task');
  });

  // ─── Inline Stop button on a running workflow card ──────────────────
  it('shows an inline Stop button on a running workflow card and calls cancelRun on click', async () => {
    const runningSummary: WorkflowSummary = {
      id: 'wf-run',
      name: 'RunningAlpha',
      project_id: null,
      project_name: null,
      trigger_type: 'manual',
      step_count: 1,
      misconfigured_step_count: 0,
      enabled: true,
      pinned: false,
      last_run: {
        id: 'run-abc',
        status: 'Running',
        started_at: '2026-01-01T00:00:00Z',
        finished_at: null,
        tokens_used: 0,
      },
      created_at: '2026-01-01T00:00:00Z',
    };
    const idleSummary: WorkflowSummary = {
      id: 'wf-idle',
      name: 'IdleBeta',
      project_id: null,
      project_name: null,
      trigger_type: 'manual',
      step_count: 1,
      misconfigured_step_count: 0,
      enabled: true,
      pinned: false,
      last_run: {
        id: 'run-xyz',
        status: 'Success',
        started_at: '2026-01-01T00:00:00Z',
        finished_at: '2026-01-01T00:10:00Z',
        tokens_used: 42,
      },
      created_at: '2026-01-01T00:00:00Z',
    };
    mockWorkflowsApi.list.mockResolvedValue([runningSummary, idleSummary]);
    mockWorkflowsApi.cancelRun.mockClear();

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // The running card renders a Stop button; the idle card does NOT.
    // There is exactly one inline .wf-card-stop-btn in the DOM.
    const stopButtons = document.querySelectorAll('.wf-card-stop-btn');
    expect(stopButtons.length).toBe(1);

    await act(async () => { fireEvent.click(stopButtons[0]); });
    await waitFor(() => expect(mockWorkflowsApi.cancelRun).toHaveBeenCalledTimes(1));
    expect(mockWorkflowsApi.cancelRun).toHaveBeenCalledWith('wf-run', 'run-abc');
  });

  it('inline Stop click does not open the workflow detail panel', async () => {
    const runningSummary: WorkflowSummary = {
      id: 'wf-run',
      name: 'RunningAlpha',
      project_id: null,
      project_name: null,
      trigger_type: 'manual',
      step_count: 1,
      misconfigured_step_count: 0,
      enabled: true,
      pinned: false,
      last_run: {
        id: 'run-abc',
        status: 'Running',
        started_at: '2026-01-01T00:00:00Z',
        finished_at: null,
        tokens_used: 0,
      },
      created_at: '2026-01-01T00:00:00Z',
    };
    mockWorkflowsApi.list.mockResolvedValue([runningSummary]);
    // If openDetail fires, workflows.get would be called — we assert it is NOT.
    mockWorkflowsApi.get.mockClear();
    mockWorkflowsApi.cancelRun.mockClear();

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    const stopButton = document.querySelector('.wf-card-stop-btn');
    expect(stopButton).not.toBeNull();
    await act(async () => { fireEvent.click(stopButton!); });

    await waitFor(() => expect(mockWorkflowsApi.cancelRun).toHaveBeenCalled());
    // openDetail would have fetched the full Workflow — it must not have.
    expect(mockWorkflowsApi.get).not.toHaveBeenCalled();
  });

  it('Delete workflow button asks for confirmation before calling the API', async () => {
    // Pre-fix: the red trash button on each workflow card called
    // `workflowsApi.delete` instantly. A mis-click destroyed the
    // workflow + every run + every child discussion. Now an explicit
    // `confirm()` is required.
    const summary: WorkflowSummary = {
      id: 'wf-del',
      name: 'DeleteMe',
      project_id: null,
      project_name: null,
      trigger_type: 'manual',
      step_count: 1,
      misconfigured_step_count: 0,
      enabled: true,
      pinned: false,
      last_run: null,
      created_at: '2026-01-01T00:00:00Z',
    };
    mockWorkflowsApi.list.mockResolvedValue([summary]);
    mockWorkflowsApi.delete.mockClear();
    window.confirm = vi.fn();
    const confirmSpy = vi.spyOn(window, 'confirm').mockReturnValue(false);

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    const deleteBtn = screen.getByLabelText('Suppr. DeleteMe');
    await act(async () => { fireEvent.click(deleteBtn); });

    expect(confirmSpy).toHaveBeenCalled();
    expect(mockWorkflowsApi.delete).not.toHaveBeenCalled();
    confirmSpy.mockRestore();
  });
});

// ── 0.8.11 UX — launch modal + disabled-state comprehension ─────────────────
// The exact flow that silently failed for a real user: a cloned (disabled)
// workflow's Lancer did nothing, and a variable-less workflow shows no popup.
describe('workflow launch modal + disabled-state UX (0.8.11)', () => {
  const labWorkflow = (over: Partial<Workflow> = {}): Workflow => ({
    id: 'wf-lab',
    name: 'PR Review LAB',
    project_id: null,
    trigger: { type: 'Manual' },
    steps: [
      { name: 'prnum', agent: 'ClaudeCode', prompt_template: 'PR {{pr_number}}', mode: { type: 'Normal' } } as never,
      // 2 steps → the wizard opens in ADVANCED mode (per-step editor cards);
      // a single step falls into simple mode where the tier select isn't shown.
      { name: 'reason', agent: 'ClaudeCode', prompt_template: 'Review {{steps.prnum.data.stdout}}', mode: { type: 'Normal' } } as never,
    ],
    actions: [],
    safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
    workspace_config: null,
    concurrency_limit: null,
    variables: [{
      name: 'pr_number', label: 'N° de la PR à reviewer', placeholder: '1800',
      description: null, required: true,
    }],
    enabled: true,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...over,
  } as Workflow);

  const labSummary = (over: Partial<WorkflowSummary> = {}): WorkflowSummary => ({
    id: 'wf-lab', name: 'PR Review LAB', project_id: null, project_name: null,
    trigger_type: 'manual', step_count: 1, misconfigured_step_count: 0,
    enabled: true, pinned: false, last_run: null, created_at: '2026-01-01T00:00:00Z', ...over,
  });

  it('switches back to workflows when an external workflow selection arrives', async () => {
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow());
    mockWorkflowsApi.listRuns.mockResolvedValue([]);
    mockWorkflowsApi.countRuns.mockResolvedValue(0);
    mockWorkflowsApi.getRun.mockResolvedValue(null);
    const onInitialSelectionConsumed = vi.fn();
    const page = await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode']}
        agentAccess={fullConfig}
      />
    );

    fireEvent.click(screen.getByRole('button', { name: /Quick Prompts/ }));
    expect(screen.getByRole('button', { name: /Quick Prompts/ })).toHaveAttribute('data-active', 'true');

    await act(async () => {
      page.rerender(
        <I18nProvider>
          <WorkflowsPage
            projects={[]}
            installedAgentTypes={['ClaudeCode']}
            agentAccess={fullConfig}
            initialSelectedWorkflowId="wf-lab"
            initialSelectedWorkflowRunId="run-from-page"
            onInitialSelectionConsumed={onInitialSelectionConsumed}
          />
        </I18nProvider>
      );
    });

    await waitFor(() => expect(mockWorkflowsApi.get).toHaveBeenCalledWith('wf-lab'));
    expect(mockWorkflowsApi.getRun).toHaveBeenCalledWith('wf-lab', 'run-from-page');
    await waitFor(() => expect(onInitialSelectionConsumed).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('button', { name: /Workflows/ })).toHaveAttribute('data-active', 'true');
    expect(screen.getByText('Éditer')).toBeInTheDocument();
  });

  it('Lancer sur un WF à variables ouvre la popup, bloque les requis vides, puis déclenche avec les valeurs', async () => {
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow());
    mockWorkflowsApi.listRuns.mockResolvedValue([]);
    mockWorkflowsApi.triggerStream.mockResolvedValue(undefined);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);

    // Click the card's Lancer (list-level trigger).
    const lancer = screen.getAllByText('Lancer')[0];
    await act(async () => { fireEvent.click(lancer); });

    // The launch modal opens with the declared variable field + required star.
    await waitFor(() => expect(screen.getByText('N° de la PR à reviewer')).toBeInTheDocument());
    const input = screen.getByPlaceholderText('1800');

    // Submit with the required field EMPTY → inline error, no trigger fired.
    const goButtons = screen.getAllByText('Lancer');
    const modalGo = goButtons[goButtons.length - 1];
    await act(async () => { fireEvent.click(modalGo); });
    expect(screen.getByText(/obligatoire/i)).toBeInTheDocument();
    expect(mockWorkflowsApi.triggerStream).not.toHaveBeenCalled();

    // Fill + submit → modal closes and the trigger fires with the value.
    fireEvent.change(input, { target: { value: '1800' } });
    await act(async () => { fireEvent.click(modalGo); });
    await waitFor(() => expect(mockWorkflowsApi.triggerStream).toHaveBeenCalled());
    const call = mockWorkflowsApi.triggerStream.mock.calls[0];
    expect(call[0]).toBe('wf-lab');
    expect(call.some((a: unknown) => !!a && typeof a === 'object' && (a as Record<string, string>).pr_number === '1800')).toBe(true);
    expect(screen.queryByText('N° de la PR à reviewer')).toBeNull();
  });

  it('masks project variables and does not ask the user to provide them', async () => {
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow({
      variables: [{
        name: 'token', label: 'API token', placeholder: '', description: null,
        required: true, source: 'project_env', source_ref: '<env.API_TOKEN>',
        allow_manual_override: false,
      }],
    } as Partial<Workflow>));
    mockWorkflowsApi.listRuns.mockResolvedValue([]);
    mockWorkflowsApi.triggerStream.mockResolvedValue(undefined);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);
    await act(async () => { fireEvent.click(screen.getAllByText('Lancer')[0]); });
    expect(await screen.findByLabelText('API token masked project value')).toHaveValue('••••••');
    expect(screen.getByText(/<env\.API_TOKEN>/)).toBeInTheDocument();
    const buttons = screen.getAllByText('Lancer');
    await act(async () => { fireEvent.click(buttons[buttons.length - 1]); });
    await waitFor(() => expect(mockWorkflowsApi.triggerStream).toHaveBeenCalled());
  });

  it('la popup valide le pattern déclaré AVANT de fermer (le rejet backend était invisible)', async () => {
    mockWorkflowsApi.triggerStream.mockClear();
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow({
      variables: [{
        name: 'pr_number', label: 'N° de la PR à reviewer', placeholder: '1800',
        description: null, required: true, pattern: '\\d+',
      }],
    } as Partial<Workflow>));
    mockWorkflowsApi.listRuns.mockResolvedValue([]);
    mockWorkflowsApi.triggerStream.mockResolvedValue(undefined);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);
    await act(async () => { fireEvent.click(screen.getAllByText('Lancer')[0]); });
    await waitFor(() => expect(screen.getByText('N° de la PR à reviewer')).toBeInTheDocument());

    // A value violating the pattern → inline error, modal stays open, no trigger.
    fireEvent.change(screen.getByPlaceholderText('1800'), { target: { value: 'PR-1800' } });
    const goButtons = screen.getAllByText('Lancer');
    const modalGo = goButtons[goButtons.length - 1];
    await act(async () => { fireEvent.click(modalGo); });
    expect(screen.getByText(/format attendu/i)).toBeInTheDocument();
    expect(mockWorkflowsApi.triggerStream).not.toHaveBeenCalled();

    // Conforming value → fires.
    fireEvent.change(screen.getByPlaceholderText('1800'), { target: { value: '1800' } });
    await act(async () => { fireEvent.click(modalGo); });
    await waitFor(() => expect(mockWorkflowsApi.triggerStream).toHaveBeenCalled());
  });

  it("un échec du trigger côté carte remonte un toast d'erreur (plus de clic silencieux)", async () => {
    mockWorkflowsApi.triggerStream.mockClear();
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    // No variables (declared OR auto-detectable) → Lancer fires directly, no modal.
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow({
      variables: [],
      steps: [
        { name: 'prnum', agent: 'ClaudeCode', prompt_template: 'Review the PR', mode: { type: 'Normal' } } as never,
        { name: 'reason', agent: 'ClaudeCode', prompt_template: 'Deep review {{steps.prnum.data.stdout}}', mode: { type: 'Normal' } } as never,
      ],
    } as Partial<Workflow>));
    mockWorkflowsApi.listRuns.mockResolvedValue([]);
    // triggerStream invokes its onError callback (SSE `error` event, e.g.
    // concurrency limit) — the 4th positional arg of triggerStream.
    mockWorkflowsApi.triggerStream.mockImplementation(
      async (_id: string, _s: unknown, _d: unknown, _done: unknown, onError: (e: string) => void) => {
        onError('Concurrency limit reached (2/2)');
      },
    );
    const toast = vi.fn();

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} toast={toast} />);
    await act(async () => { fireEvent.click(screen.getAllByText('Lancer')[0]); });

    await waitFor(() => expect(toast).toHaveBeenCalled());
    const [msg, kind] = toast.mock.calls[0];
    expect(String(msg)).toMatch(/Concurrency limit/);
    expect(kind).toBe('error');
  });

  it('carte désactivée : Lancer est inerte MAIS porte le tooltip explicatif', async () => {
    mockWorkflowsApi.list.mockResolvedValue([labSummary({ enabled: false })]);
    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);

    const lancer = screen.getAllByText('Lancer')[0].closest('button');
    expect(lancer).toBeDisabled();
    expect(lancer?.getAttribute('title')).toMatch(/désactivé/i);
  });

  it('charge 10 runs au départ puis respecte le choix 50', async () => {
    const run = (id: string) => ({
      id,
      workflow_id: 'wf-lab',
      status: 'Success',
      trigger_context: null,
      step_results: [],
      tokens_used: 0,
      workspace_path: null,
      started_at: `2026-07-24T10:${id.padStart(2, '0')}:00Z`,
      finished_at: `2026-07-24T10:${id.padStart(2, '0')}:30Z`,
      run_type: 'linear',
      batch_total: 0,
      batch_completed: 0,
      batch_failed: 0,
      batch_name: null,
      parent_run_id: null,
      state: {},
      produced_branches: [],
    });
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow());
    mockWorkflowsApi.listRuns.mockReset();
    mockWorkflowsApi.countRuns.mockReset();
    mockWorkflowsApi.countRuns.mockResolvedValue(60);
    mockWorkflowsApi.listRuns
      .mockResolvedValueOnce(Array.from({ length: 10 }, (_, index) => run(String(index))))
      .mockResolvedValueOnce(Array.from({ length: 50 }, (_, index) => run(String(index + 10))));

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Ouvrir PR Review LAB' })); });
    await waitFor(() => expect(screen.getByText('Runs (60)')).toBeInTheDocument());
    expect(mockWorkflowsApi.listRuns).toHaveBeenNthCalledWith(1, 'wf-lab', 10, 0, true);

    fireEvent.change(screen.getByLabelText(/Nombre d’exécutions à charger/i), {
      target: { value: '50' },
    });
    await act(async () => { fireEvent.click(screen.getByText('Afficher')); });
    await waitFor(() => {
      expect(mockWorkflowsApi.listRuns).toHaveBeenNthCalledWith(2, 'wf-lab', 50, 10, true);
    });
  });

  it('conserve le scroll de la liste et remonte le détail lors de la sélection', async () => {
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow());
    mockWorkflowsApi.listRuns.mockResolvedValue([]);
    mockWorkflowsApi.countRuns.mockResolvedValue(0);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);

    const listPane = document.querySelector<HTMLElement>('.automation-sidebar-items')!;
    listPane.scrollTop = 700;

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Ouvrir PR Review LAB' })); });

    expect(listPane.scrollTop).toBe(700);
    const detailPane = screen.getByTestId('workflow-detail-pane');
    expect(detailPane.scrollTop).toBe(0);
  });

  it("change l'agent et le mode IA d'un step en une seule sélection", async () => {
    const agentSettings = {
      model: 'claude-opus',
      tier: 'reasoning',
      reasoning_effort: 'high',
      max_tokens: null,
    } as const;
    const workflow = labWorkflow({
      steps: [
        {
          name: 'prnum',
          step_type: { type: 'Agent' },
          agent: 'ClaudeCode',
          agent_settings: agentSettings,
          prompt_template: 'PR {{pr_number}}',
          mode: { type: 'Normal' },
        } as never,
        {
          name: 'reason',
          step_type: { type: 'Agent' },
          agent: 'ClaudeCode',
          prompt_template: 'Review {{steps.prnum.data.stdout}}',
          mode: { type: 'Normal' },
        } as never,
      ],
    });
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(workflow);
    mockWorkflowsApi.listRuns.mockResolvedValue([]);
    mockWorkflowsApi.countRuns.mockResolvedValue(0);
    mockWorkflowsApi.update.mockReset().mockResolvedValue({
      ...workflow,
      steps: workflow.steps.map((step, index) =>
        index === 0 ? { ...step, agent: 'Codex' } : step
      ),
    });

    await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode', 'Codex']}
        agentAccess={fullConfig}
      />
    );
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Ouvrir PR Review LAB' })); });
    await waitFor(() => expect(screen.getByText('Éditer')).toBeInTheDocument());

    // The selected Agent step deliberately exposes the same switcher in the
    // compact pipeline and in its preview card. Exercise the preview control
    // explicitly so this regression test keeps asserting the inline editor
    // surface instead of depending on a globally unique accessible name.
    const previewPanel = screen.getByRole('tabpanel', { name: 'Aperçu' });
    fireEvent.click(within(previewPanel).getByLabelText(/Changer l'agent ou le mode IA du step « prnum »/i));
    await act(async () => { fireEvent.click(screen.getByRole('menuitem', { name: 'Codex · Avancé' })); });

    await waitFor(() => expect(mockWorkflowsApi.update).toHaveBeenCalledTimes(1));
    const [, request] = mockWorkflowsApi.update.mock.calls[0];
    expect(request.steps[0]).toMatchObject({
      agent: 'Codex',
      agent_settings: {
        ...agentSettings,
        model: null,
        tier: 'reasoning',
      },
    });
    expect(request.steps[1].agent).toBe('ClaudeCode');
  });

  it('wizard : chaque step Agent expose le sélecteur agent × mode IA et applique le changement', async () => {
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow());
    mockWorkflowsApi.listRuns.mockResolvedValue([]);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Ouvrir PR Review LAB' })); });
    await waitFor(() => expect(screen.getByText('Éditer')).toBeDefined());
    await act(async () => { fireEvent.click(screen.getByText('Éditer')); });

    // Navigate to the Steps stage — adaptive: click "Suivant" until the step
    // editor (name input 'prnum') is visible (a declared-variables workflow can
    // add a stage vs the plain flow).
    for (let i = 0; i < 5 && !screen.queryByDisplayValue('prnum'); i++) {
      const next = screen.queryAllByText('Suivant');
      if (!next.length) break;
      await act(async () => { fireEvent.click(next[next.length - 1]); });
    }
    await waitFor(() => expect(screen.getByDisplayValue('prnum')).toBeInTheDocument());

    const agentTierPicker = screen.getAllByRole('button', { name: 'Agent et mode IA' })[0];
    expect(agentTierPicker).toHaveTextContent('🎯');
    fireEvent.click(agentTierPicker);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Claude Code · Avancé' }));
    expect(agentTierPicker).toHaveTextContent('🧠');
  });

  it('uses the workflow card hierarchy for Quick Prompts', async () => {
    const qp: QuickPrompt = {
      id: 'qp-release-notes',
      pinned: false,
      name: 'Release notes',
      icon: '✍️',
      description: 'Prépare des notes de version claires et actionnables.',
      prompt_template: 'Résume {{changes}}',
      variables: [{
        name: 'changes',
        label: 'Changements',
        placeholder: 'feat: ...',
        description: null,
        required: true,
        source: 'user_input',
        source_ref: null,
        allow_manual_override: false,
      }],
      agent: 'Codex',
      project_id: null,
      skill_ids: [],
      profile_ids: [],
      directive_ids: [],
      tier: 'default',
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    };
    vi.mocked(quickPromptsApi.list).mockResolvedValueOnce([qp]);

    const { container } = await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['Codex']}
        agentAccess={fullConfig}
      />
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Quick Prompts \(1\)/ }));
    });

    const card = container.querySelector('[data-qp-id="qp-release-notes"]');
    expect(card).toHaveAttribute('data-kind', 'prompt');
    expect(card?.querySelector('.qp-card-identity')).toHaveTextContent('Release notes');
    expect(card?.querySelector('.qp-card-desc')).toHaveTextContent('Prépare des notes de version');
    expect(card?.querySelector('.qp-card-meta')).toHaveTextContent('1 variable');
    expect(card?.querySelector('.qp-card-tools')).toBeInTheDocument();
    expect(card?.querySelector('.qp-card-primary-actions')).toHaveTextContent('Lancer');
    expect(screen.getByLabelText('Éditer Release notes')).toBeInTheDocument();
  });

  it('filters Quick Prompts by agent without changing the selected sort', async () => {
    const qp = (id: string, name: string, agent: QuickPrompt['agent']): QuickPrompt => ({
      id,
      pinned: false,
      name,
      icon: '✨',
      description: '',
      prompt_template: name,
      variables: [],
      agent,
      project_id: null,
      skill_ids: [],
      profile_ids: [],
      directive_ids: [],
      tier: 'default',
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    });
    vi.mocked(quickPromptsApi.list).mockResolvedValueOnce([
      qp('qp-codex', 'Codex prompt', 'Codex'),
      qp('qp-claude', 'Claude prompt', 'ClaudeCode'),
    ]);

    const { container } = await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode', 'Codex']}
        agentAccess={fullConfig}
      />
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Quick Prompts \(2\)/ }));
    });

    fireEvent.change(screen.getByLabelText('Filtrer les Quick Prompts par agent'), {
      target: { value: 'Codex' },
    });
    expect(container.querySelector('[data-qp-id="qp-codex"]')).toBeInTheDocument();
    expect(container.querySelector('[data-qp-id="qp-claude"]')).not.toBeInTheDocument();
    expect(screen.getByLabelText('Trier les Quick Prompts')).toHaveValue('name');
  });

  it('changes a Quick Prompt agent and AI mode together from its card', async () => {
    const qp: QuickPrompt = {
      id: 'qp-agent-tier',
      pinned: false,
      name: 'Review release',
      icon: '🔎',
      description: '',
      prompt_template: 'Review',
      variables: [],
      agent: 'ClaudeCode',
      project_id: null,
      skill_ids: [],
      profile_ids: [],
      directive_ids: [],
      tier: 'default',
      agent_settings: { model: 'opus', tier: 'default', reasoning_effort: null, max_tokens: null },
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    };
    vi.mocked(quickPromptsApi.list).mockResolvedValueOnce([qp]);
    vi.mocked(quickPromptsApi.update).mockReset().mockResolvedValue(qp);

    await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode', 'Codex']}
        agentAccess={fullConfig}
      />,
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Quick Prompts \(1\)/ }));
    });

    const trigger = screen.getByRole('button', {
      name: /Changer l'agent ou le mode IA du QP « Review release »/,
    });
    expect(trigger).toHaveTextContent('🎯');
    expect(trigger).toHaveAttribute('title', expect.stringContaining('sonnet'));
    fireEvent.click(trigger);
    await act(async () => {
      fireEvent.click(screen.getByRole('menuitem', { name: 'Codex · Avancé' }));
    });

    await waitFor(() => expect(quickPromptsApi.update).toHaveBeenCalledWith(
      'qp-agent-tier',
      expect.objectContaining({
        agent: 'Codex',
        tier: 'reasoning',
        agent_settings: expect.objectContaining({ model: null, tier: 'reasoning' }),
      }),
    ));
  });

  it('offers AI-assisted creation and global JSON import from the Quick APIs tab', async () => {
    const onNavigateDiscussion = vi.fn();
    vi.mocked(discussionsApi.create).mockResolvedValueOnce({ id: 'disc-qa-architect' } as never);
    vi.mocked(quickApisApi.importQa).mockResolvedValueOnce({ id: 'qa-imported' } as never);

    const { container } = await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode']}
        agentAccess={fullConfig}
        onNavigateDiscussion={onNavigateDiscussion}
      />
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Quick APIs \(0\)/ }));
    });

    const actionDialog = openAutomationActions();
    expect(within(actionDialog).getByRole('button', { name: 'Importer' })).toHaveAttribute(
      'title',
      expect.stringContaining('Le type est détecté automatiquement'),
    );
    await act(async () => {
      fireEvent.click(within(actionDialog).getByRole('button', { name: /Créer avec l'IA/ }));
    });

    await waitFor(() => expect(discussionsApi.create).toHaveBeenCalledWith(
      expect.objectContaining({
        title: 'Architecte de Quick API',
        initial_prompt: expect.stringContaining('qa_create_draft'),
        skill_ids: [],
        tier: 'reasoning',
      }),
    ));
    expect(onNavigateDiscussion).toHaveBeenCalledWith('disc-qa-architect');

    chooseAutomationAction('Importer');
    const globalModalTitle = screen.getByRole('heading', { name: 'Importer dans Automation' });
    const modal = globalModalTitle.closest('.wf-import-modal');
    expect(modal).not.toBeNull();
    expect(globalModalTitle).toBeInTheDocument();
    expect(screen.getByText(/\.kronn-qa\.json/)).toBeInTheDocument();

    const content = JSON.stringify({
      kind: 'kronn.quick_api',
      version: 1,
      quick_api: {
        name: 'Imported QA',
        variables: [{ name: 'ticket' }],
      },
    });
    const file = new File([content], 'imported.kronn-qa.json', { type: 'application/json' });
    Object.defineProperty(file, 'text', { value: vi.fn().mockResolvedValue(content) });
    const fileInput = container.querySelector<HTMLInputElement>('input[type="file"]');
    expect(fileInput).not.toBeNull();
    await act(async () => {
      fireEvent.change(fileInput!, { target: { files: [file] } });
    });
    await waitFor(() => expect(screen.getByText('Imported QA')).toBeInTheDocument());
    expect(screen.getByRole('heading', { name: 'Importer un Quick API' })).toBeInTheDocument();

    await act(async () => {
      fireEvent.click(within(modal as HTMLElement).getByRole('button', { name: 'Importer' }));
    });
    await waitFor(() => expect(quickApisApi.importQa).toHaveBeenCalledWith({
      content,
      project_id: null,
    }));
  });

  it('keeps the global importer available while an Automation resource is selected', async () => {
    const qp: QuickPrompt = {
      id: 'qp-selected', name: 'Selected prompt', icon: '🧪', description: '',
      pinned: false,
      prompt_template: 'Test', variables: [], agent: 'ClaudeCode', project_id: null,
      skill_ids: [], profile_ids: [], directive_ids: [], tier: 'default', agent_settings: null,
      created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    };
    vi.mocked(quickPromptsApi.list).mockResolvedValueOnce([qp]);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={[]} agentAccess={fullConfig} />);
    fireEvent.click(screen.getByRole('button', { name: /Quick Prompts \(1\)/ }));
    fireEvent.click(screen.getByRole('button', { name: 'Ouvrir Selected prompt' }));

    const actionDialog = openAutomationActions();
    expect(within(actionDialog).getByRole('button', { name: 'Importer' })).toBeInTheDocument();
  });

  it('routes Quick Prompt and Quick Exec files through their existing import APIs', async () => {
    vi.mocked(quickPromptsApi.importQp).mockResolvedValueOnce({ id: 'qp-imported' } as never);
    vi.mocked(quickExecsApi.import).mockResolvedValueOnce({ id: 'qe-imported' } as never);
    const { container } = await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={[]} agentAccess={fullConfig} />,
    );

    const importFile = async (content: string, filename: string, expectedTitle: string) => {
      chooseAutomationAction('Importer');
      const file = new File([content], filename, { type: 'application/json' });
      Object.defineProperty(file, 'text', { value: vi.fn().mockResolvedValue(content) });
      await act(async () => {
        fireEvent.change(container.querySelector<HTMLInputElement>('input[type="file"]')!, {
          target: { files: [file] },
        });
      });
      const modal = screen.getByRole('heading', { name: expectedTitle }).closest('.wf-import-modal');
      await act(async () => {
        fireEvent.click(within(modal as HTMLElement).getByRole('button', { name: 'Importer' }));
      });
    };

    const qpContent = JSON.stringify({
      kind: 'kronn.quick_prompt', quick_prompt: { name: 'Imported prompt', variables: [] },
    });
    await importFile(qpContent, 'prompt.json', 'Importer un Quick Prompt');
    await waitFor(() => expect(quickPromptsApi.importQp).toHaveBeenCalledWith({
      content: qpContent,
      project_id: null,
    }));

    const qeContent = JSON.stringify({
      kind: 'kronn.quick_exec', quick_exec: { name: 'Imported exec', variables: [] },
    });
    await importFile(qeContent, 'exec.json', 'Importer un Quick Exec');
    await waitFor(() => expect(quickExecsApi.import).toHaveBeenCalledWith({
      content: qeContent,
      project_id: null,
    }));
  });

  it('shows explicit errors for invalid and ambiguous Automation exports', async () => {
    const { container } = await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={[]} agentAccess={fullConfig} />,
    );
    chooseAutomationAction('Importer');
    const input = container.querySelector<HTMLInputElement>('input[type="file"]')!;

    const invalidContent = JSON.stringify({ kind: 'kronn.unknown', item: {} });
    const invalidFile = new File([invalidContent], 'invalid.json', { type: 'application/json' });
    Object.defineProperty(invalidFile, 'text', { value: vi.fn().mockResolvedValue(invalidContent) });
    await act(async () => {
      fireEvent.change(input, { target: { files: [invalidFile] } });
    });
    expect(await screen.findByText(/Export Kronn non pris en charge/)).toBeInTheDocument();

    const ambiguousContent = JSON.stringify({
      kind: 'kronn.workflow',
      workflow: { name: 'Workflow' },
      quick_exec: { name: 'Exec' },
    });
    const ambiguousFile = new File([ambiguousContent], 'ambiguous.json', { type: 'application/json' });
    Object.defineProperty(ambiguousFile, 'text', { value: vi.fn().mockResolvedValue(ambiguousContent) });
    await act(async () => {
      fireEvent.change(input, { target: { files: [ambiguousFile] } });
    });
    expect(await screen.findByText(/Export Kronn ambigu/)).toBeInTheDocument();
  });

  it('dispatches the Pages capability reconciliation after a workflow import', async () => {
    mockWorkflowsApi.importWorkflow.mockResolvedValueOnce({ id: 'wf-imported' });
    mockWorkflowsApi.get.mockResolvedValueOnce({
      id: 'wf-imported', name: 'Imported workflow', project_id: null,
      trigger: { type: 'Manual' }, steps: [], actions: [], variables: [], enabled: false,
      pinned: false, created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
    });
    const activated = vi.fn();
    window.addEventListener('kronn:pages-activated', activated, { once: true });

    const { container } = await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={[]} agentAccess={fullConfig} />,
    );
    chooseAutomationAction('Importer');
    const content = JSON.stringify({
      kind: 'kronn.workflow',
      version: 3,
      workflow: { name: 'Imported workflow', steps: [] },
      referenced_pages: [{ id: 'page-old', slug: 'ops', title: 'Ops', html: '<main />', datasets: [] }],
    });
    const file = new File([content], 'workflow.json', { type: 'application/json' });
    Object.defineProperty(file, 'text', { value: vi.fn().mockResolvedValue(content) });
    await act(async () => {
      fireEvent.change(container.querySelector<HTMLInputElement>('input[type="file"]')!, {
        target: { files: [file] },
      });
    });
    const modal = screen.getByRole('heading', { name: 'Importer un workflow' }).closest('.wf-import-modal');
    await act(async () => {
      fireEvent.click(within(modal as HTMLElement).getByRole('button', { name: 'Importer' }));
    });

    await waitFor(() => expect(activated).toHaveBeenCalledOnce());
    expect(mockWorkflowsApi.importWorkflow).toHaveBeenCalledWith({ content, project_id: null });
    window.removeEventListener('kronn:pages-activated', activated);
  });

  it('groups Quick APIs by API and sorts each group by endpoint', async () => {
    const qa = (
      id: string,
      name: string,
      plugin: string,
      endpoint: string,
    ): QuickApi => ({
      id,
      pinned: false,
      name,
      icon: '⚡',
      description: '',
      project_id: null,
      api_plugin_slug: plugin,
      api_config_id: `${plugin}-config`,
      api_endpoint_path: endpoint,
      variables: [],
      profile_ids: [],
      directive_ids: [],
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
    });
    vi.mocked(quickApisApi.list).mockResolvedValueOnce([
      qa('jira-z', 'Create ticket', 'jira', '/tickets/z'),
      qa('chartbeat-z', 'Top pages', 'chartbeat', '/top'),
      qa('jira-a', 'Find ticket', 'jira', '/tickets/a'),
    ]);

    const { container } = await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={[]} agentAccess={fullConfig} />
    );
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Quick APIs \(3\)/ }));
    });
    fireEvent.change(screen.getByLabelText('Trier les Quick APIs'), {
      target: { value: 'endpoint' },
    });

    const rowOrder = () => Array.from(container.querySelectorAll('.qp-list > *')).map(row =>
      row.classList.contains('quick-api-group-heading')
        ? `group:${row.textContent}`
        : `qa:${row.getAttribute('data-qa-id')}`
    );
    expect(rowOrder()).toEqual([
      'group:chartbeatAPI',
      'qa:chartbeat-z',
      'group:jiraAPI',
      'qa:jira-a',
      'qa:jira-z',
    ]);

    fireEvent.click(screen.getByRole('button', { name: 'Inverser l’ordre' }));
    expect(rowOrder()).toEqual([
      'group:jiraAPI',
      'qa:jira-z',
      'qa:jira-a',
      'group:chartbeatAPI',
      'qa:chartbeat-z',
    ]);
    expect(screen.getByRole('button', { name: 'Rétablir l’ordre par défaut' }))
      .toHaveAttribute('aria-pressed', 'true');

    fireEvent.change(screen.getByLabelText('Filtrer les Quick APIs par API'), {
      target: { value: 'jira' },
    });
    expect(rowOrder()).toEqual([
      'group:jiraAPI',
      'qa:jira-z',
      'qa:jira-a',
    ]);

    const jiraCard = container.querySelector('[data-qa-id="jira-a"]');
    expect(jiraCard).toHaveAttribute('data-kind', 'api');
    expect(jiraCard?.querySelector('.qp-card-identity')).toHaveTextContent('Find ticket');
    expect(jiraCard?.querySelector('.qp-card-api-plugin')).toHaveTextContent('jira');
    expect(jiraCard?.querySelector('.qp-card-endpoint')).toHaveTextContent('GET/tickets/a');
    expect(jiraCard?.querySelector('.qp-card-tools')).toBeInTheDocument();
    expect(jiraCard?.querySelector('.qp-card-primary-actions')).toHaveTextContent('Lancer');
  });
});

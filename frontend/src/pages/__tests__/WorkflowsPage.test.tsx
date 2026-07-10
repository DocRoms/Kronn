import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { WorkflowsPage } from '../WorkflowsPage';
import type { AgentsConfig, Workflow, WorkflowSummary } from '../../types/generated';

const mockWorkflowsApi = vi.hoisted(() => ({
  list: vi.fn().mockResolvedValue([]),
  get: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
  trigger: vi.fn(),
  listRuns: vi.fn().mockResolvedValue([]),
  getRun: vi.fn(),
  deleteRun: vi.fn(),
  deleteAllRuns: vi.fn(),
  cancelRun: vi.fn().mockResolvedValue({ run_cancelled: true, child_discs_cancelled: 0 }),
  triggerStream: vi.fn(),
}));

// 0.8.2 — WorkflowsPage now uses useWebSocket() to listen for live
// WorkflowRunUpdated events. Stub it to a no-op so the test runtime
// doesn't try to open a real WebSocket inside jsdom.
vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: () => ({ connected: false }),
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
  quickPrompts: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
    batchRun: vi.fn(),
  },
  quickApis: {
    list: vi.fn().mockResolvedValue([]),
    create: vi.fn(),
    update: vi.fn(),
    delete: vi.fn(),
    runQa: vi.fn(),
    exportQa: vi.fn(),
    importQa: vi.fn(),
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
};

const restrictedConfig: AgentsConfig = {
  claude_code: { path: null, installed: true, version: null, full_access: false },
  codex: { path: null, installed: true, version: null, full_access: false },
  gemini_cli: { path: null, installed: true, version: null, full_access: false },
  kiro: { path: null, installed: false, version: null, full_access: false },
  vibe: { path: null, installed: false, version: null, full_access: false },
  copilot_cli: { path: null, installed: false, version: null, full_access: false },
  ollama: { path: null, installed: false, version: null, full_access: false },
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
  model_tiers: defaultModelTiers,
};

afterEach(cleanup);

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
};

describe('WorkflowsPage', () => {
  it('renders with various agentAccess configs and shows create button', async () => {
    // Without agentAccess
    const { unmount: u1 } = await wrap(<WorkflowsPage projects={[]} />);
    expect(screen.getByText('Automatisation')).toBeDefined();
    expect(screen.getByText('Nouveau workflow')).toBeDefined();
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
    expect(screen.getByText('Nouveau workflow')).toBeDefined();
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
    expect(screen.getByText('Nouveau workflow')).toBeDefined();
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
    expect(screen.getByText('Nouveau workflow')).toBeDefined();

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
        { name: 'analyze', agent: 'ClaudeCode', prompt_template: 'Analyse this bug', mode: { type: 'Normal' } },
        { name: 'fix', agent: 'Codex', prompt_template: 'Fix: {{previous_step.output}}', mode: { type: 'Normal' } },
      ],
      actions: [],
      safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
      workspace_config: null,
      concurrency_limit: null,
      enabled: true,
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
    const workflowCard = screen.getByText('My Workflow');
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

    await waitFor(() => expect(screen.getByText('Ticket → PR')).toBeDefined());
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

    await waitFor(() => expect(screen.getByText('Clean WF')).toBeDefined());
    expect(screen.queryByText(/à configurer/)).toBeNull();
  });

  // ─── Wizard validation errors on summary page ───────────────────────────

  it('shows validation error for missing prompt on summary step (simple mode)', async () => {
    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Click "Nouveau workflow" to open wizard
    const newBtn = screen.getByText(/Nouveau workflow/);
    await act(async () => { fireEvent.click(newBtn); });

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
    const newBtn = screen.getByText(/Nouveau workflow/);
    await act(async () => { fireEvent.click(newBtn); });

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
    const newBtn = screen.getByText(/Nouveau workflow/);
    await act(async () => { fireEvent.click(newBtn); });

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
    const newBtn = screen.getByText(/Nouveau workflow/);
    await act(async () => { fireEvent.click(newBtn); });

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

    const deleteBtn = document.querySelector('.wf-small-btn-danger') as HTMLButtonElement;
    expect(deleteBtn).not.toBeNull();
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
    enabled: true, last_run: null, created_at: '2026-01-01T00:00:00Z', ...over,
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

  it('wizard : chaque step Agent expose le sélecteur de palier (⚡/🎯/🧠) et le changement est appliqué', async () => {
    mockWorkflowsApi.list.mockResolvedValue([labSummary()]);
    mockWorkflowsApi.get.mockResolvedValue(labWorkflow());
    mockWorkflowsApi.listRuns.mockResolvedValue([]);

    await wrap(<WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />);
    await act(async () => { fireEvent.click(screen.getByText('PR Review LAB')); });
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

    const tierSelect = screen.getAllByTitle(/Palier de modèle/)[0] as HTMLSelectElement;
    expect(tierSelect.value).toBe('default');
    expect(Array.from(tierSelect.options).map(o => o.value)).toEqual(['economy', 'default', 'reasoning']);
    fireEvent.change(tierSelect, { target: { value: 'reasoning' } });
    expect(tierSelect.value).toBe('reasoning');
  });
});

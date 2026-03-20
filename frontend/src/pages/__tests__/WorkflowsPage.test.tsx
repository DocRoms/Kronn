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
  triggerStream: vi.fn(),
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
}));

const defaultModelTiers = {
  claude_code: { economy: null, reasoning: null },
  codex: { economy: null, reasoning: null },
  gemini_cli: { economy: null, reasoning: null },
  kiro: { economy: null, reasoning: null },
  vibe: { economy: null, reasoning: null },
};

const restrictedConfig: AgentsConfig = {
  claude_code: { path: null, installed: true, version: null, full_access: false },
  codex: { path: null, installed: true, version: null, full_access: false },
  gemini_cli: { path: null, installed: true, version: null, full_access: false },
  kiro: { path: null, installed: false, version: null, full_access: false },
  vibe: { path: null, installed: false, version: null, full_access: false },
  model_tiers: defaultModelTiers,
};

const fullConfig: AgentsConfig = {
  claude_code: { path: null, installed: true, version: null, full_access: true },
  codex: { path: null, installed: true, version: null, full_access: true },
  gemini_cli: { path: null, installed: true, version: null, full_access: true },
  kiro: { path: null, installed: false, version: null, full_access: true },
  vibe: { path: null, installed: false, version: null, full_access: true },
  model_tiers: defaultModelTiers,
};

afterEach(cleanup);

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  return result!;
};

describe('WorkflowsPage', () => {
  it('renders with various agentAccess configs and shows create button', async () => {
    // Without agentAccess
    const { unmount: u1 } = await wrap(<WorkflowsPage projects={[]} />);
    expect(screen.getByText('Workflows')).toBeDefined();
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
    expect(screen.getByText('Workflows')).toBeDefined();
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
    expect(screen.getByText('Workflows')).toBeDefined();
    expect(screen.getByText('Nouveau workflow')).toBeDefined();
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
      expect(screen.getByText('Editer')).toBeDefined();
    });

    await act(async () => {
      fireEvent.click(screen.getByText('Editer'));
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

  // ─── Wizard validation errors on summary page ───────────────────────────

  it('shows validation error for missing prompt on summary step', async () => {
    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Click "Nouveau workflow" to open wizard
    const newBtn = screen.getByText(/Nouveau workflow/);
    await act(async () => { fireEvent.click(newBtn); });

    // Fill workflow name on step 0 (required to navigate past step 0)
    const nameInput = screen.getByPlaceholderText('ex: Auto-fix 5xx errors');
    await act(async () => { fireEvent.change(nameInput, { target: { value: 'Test WF' } }); });

    // Navigate to summary step (step 4): click "Suivant" 4 times (0→1→2→3→4)
    for (let i = 0; i < 4; i++) {
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

  it('creates a workflow through the wizard', async () => {
    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Click "Nouveau workflow" to open wizard
    const newBtn = screen.getByText(/Nouveau workflow/);
    await act(async () => { fireEvent.click(newBtn); });

    // Step 0: fill the workflow name
    const nameInput = screen.getByPlaceholderText('ex: Auto-fix 5xx errors');
    await act(async () => { fireEvent.change(nameInput, { target: { value: 'My CI Workflow' } }); });

    // The "Suivant" button should now be enabled
    let nextBtns = screen.getAllByText(/Suivant/);
    let nextBtn = nextBtns[nextBtns.length - 1];
    expect(nextBtn.closest('button')!.disabled).toBe(false);

    // Navigate to step 1 (trigger)
    await act(async () => { fireEvent.click(nextBtn); });

    // Step 1 should show trigger options — verify "Manual" is visible
    expect(document.body.textContent).toContain('Manual');

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
});

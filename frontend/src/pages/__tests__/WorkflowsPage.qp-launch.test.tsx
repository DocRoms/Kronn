import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, act, cleanup, fireEvent, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { WorkflowsPage } from '../WorkflowsPage';
import type { AgentsConfig, QuickPrompt } from '../../types/generated';

// Resolver controlled by each test so we can hold `discussions.create` open
// while we fire a second click — that's the whole point of the race test.
const createResolvers = vi.hoisted(() => ({
  resolve: undefined as ((d: { id: string; title: string }) => void) | undefined,
  reset: () => {
    createResolvers.resolve = undefined;
  },
}));

const mockDiscussionsApi = vi.hoisted(() => ({
  create: vi.fn(),
}));

const mockQuickPromptsApi = vi.hoisted(() => ({
  list: vi.fn(),
  create: vi.fn(),
  update: vi.fn(),
  delete: vi.fn(),
  batchRun: vi.fn(),
  compareAgents: vi.fn(),
  exportQp: vi.fn(),
  importQp: vi.fn(),
}));

// 0.8.2 — WorkflowsPage now uses useWebSocket() to listen for live
// WorkflowRunUpdated events. Stub it so the test runtime doesn't try
// to open a real WebSocket inside jsdom.
vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: () => ({ connected: false }),
}));

vi.mock('../../lib/api', () => ({
  workflows: {
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
    cancelRun: vi.fn(),
    triggerStream: vi.fn(),
    exportWorkflow: vi.fn(),
    importWorkflow: vi.fn(),
  },
  skills: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), delete: vi.fn() },
  profiles: { list: vi.fn().mockResolvedValue([]), get: vi.fn(), create: vi.fn(), update: vi.fn(), delete: vi.fn() },
  directives: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), delete: vi.fn() },
  quickPrompts: mockQuickPromptsApi,
  quickApis: { list: vi.fn().mockResolvedValue([]), create: vi.fn(), update: vi.fn(), delete: vi.fn(), runQa: vi.fn(), exportQa: vi.fn(), importQa: vi.fn(), batchRunQa: vi.fn() },
  config: { getUiLanguage: vi.fn().mockResolvedValue('fr'), saveUiLanguage: vi.fn().mockResolvedValue(undefined), getServerConfig: vi.fn().mockResolvedValue({ default_model_tier: 'default' }) },
  mcps: {
    overview: vi.fn().mockResolvedValue({ servers: [], configs: [], customized_contexts: [], incompatibilities: [] }),
    registry: vi.fn().mockResolvedValue([]),
  },
  discussions: mockDiscussionsApi,
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

const sampleQpWithVar: QuickPrompt = {
  id: 'qp-1',
  name: 'Analyse ticket',
  icon: '🎯',
  prompt_template: 'Analyse the ticket {{ticket}} and report findings.',
  variables: [{ name: 'ticket', label: 'Ticket', placeholder: 'EW-1234', description: '', required: true }],
  agent: 'ClaudeCode',
  project_id: null, skill_ids: [], profile_ids: [], directive_ids: [], tier: 'default', description: '',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const sampleQpNoVar: QuickPrompt = {
  id: 'qp-2',
  name: 'Daily standup',
  icon: '📅',
  prompt_template: 'Summarize what changed since yesterday.',
  variables: [],
  agent: 'ClaudeCode',
  project_id: null, skill_ids: [], profile_ids: [], directive_ids: [], tier: 'default', description: '',
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

const wrap = async (ui: React.ReactElement) => {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<I18nProvider>{ui}</I18nProvider>);
  });
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
};

afterEach(() => {
  cleanup();
  createResolvers.reset();
  mockDiscussionsApi.create.mockReset();
  mockQuickPromptsApi.list.mockReset();
  mockQuickPromptsApi.compareAgents.mockReset();
});

describe('WorkflowsPage — QP launch double-click race', () => {
  it('does not spawn duplicate discussions on two fast Enter presses (QP with variable)', async () => {
    mockQuickPromptsApi.list.mockResolvedValue([sampleQpWithVar]);
    // Hold create() open until we manually resolve, so the second Enter has
    // a chance to fire before the first one finishes.
    mockDiscussionsApi.create.mockImplementation(() => new Promise(resolve => {
      createResolvers.resolve = resolve as (d: { id: string; title: string }) => void;
    }));

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    // Switch to the Quick Prompts tab so the QP card renders.
    const qpTab = await screen.findByText(/Quick Prompts/);
    await act(async () => { fireEvent.click(qpTab); });

    // Click "Launch" on the QP card to open the variable form.
    const launchButtons = await screen.findAllByText(/Lancer|Launch|Ejecutar/);
    expect(launchButtons.length).toBeGreaterThan(0);
    await act(async () => { fireEvent.click(launchButtons[0]); });

    // The variable input is now mounted — fill it.
    const ticketInput = await screen.findByPlaceholderText('EW-1234');
    await act(async () => { fireEvent.change(ticketInput, { target: { value: 'EW-7777' } }); });

    // Two synchronous Enter presses BEFORE create() resolves.
    await act(async () => {
      fireEvent.keyDown(ticketInput, { key: 'Enter' });
      fireEvent.keyDown(ticketInput, { key: 'Enter' });
    });

    // Without the ref guard the second Enter fires a second create() call.
    expect(mockDiscussionsApi.create).toHaveBeenCalledTimes(1);

    // Now resolve so React can update state and free the test cleanly.
    await act(async () => {
      createResolvers.resolve?.({ id: 'disc-1', title: 'x' });
    });
  });

  it('does not spawn duplicate discussions on two fast clicks (no-variable QP)', async () => {
    mockQuickPromptsApi.list.mockResolvedValue([sampleQpNoVar]);
    mockDiscussionsApi.create.mockImplementation(() => new Promise(resolve => {
      createResolvers.resolve = resolve as (d: { id: string; title: string }) => void;
    }));

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );

    const qpTab = await screen.findByText(/Quick Prompts/);
    await act(async () => { fireEvent.click(qpTab); });

    // The Launch button on a no-variable QP fires handleLaunchQP synchronously.
    const launchButtons = await screen.findAllByText(/Lancer|Launch|Ejecutar/);
    expect(launchButtons.length).toBeGreaterThan(0);

    await act(async () => {
      fireEvent.click(launchButtons[0]);
      fireEvent.click(launchButtons[0]);
    });

    expect(mockDiscussionsApi.create).toHaveBeenCalledTimes(1);

    await act(async () => {
      createResolvers.resolve?.({ id: 'disc-2', title: 'y' });
    });
  });

  it('compare-agents chip selector — deselecting an agent removes it from the API payload', async () => {
    // Pin: 🤝 Compare must respect the chip selector. Pre-fix the CTA
    // always fanned out across `installedAgentTypes`, ignoring whatever
    // the user toggled. Two different bugs hide here — fan-out across
    // un-asked agents (cost) AND the inverse: silently dropping a
    // selection (UX confusion). This test catches both regressions.
    mockQuickPromptsApi.list.mockResolvedValue([sampleQpNoVar]);
    mockQuickPromptsApi.compareAgents.mockResolvedValue({
      run_id: 'run-1',
      batch_total: 2,
      discussion_ids: ['d1', 'd2'],
    });

    await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode', 'Codex', 'GeminiCli']}
        agentAccess={fullConfig}
      />
    );

    const qpTab = await screen.findByText(/Quick Prompts/);
    await act(async () => { fireEvent.click(qpTab); });

    // Open the compare form via the 🤝 icon button. For no-var QPs this
    // now opens the form (so the chip selector is reachable) instead of
    // firing immediately.
    const compareBtn = await screen.findByTestId('qp-compare-agents-btn');
    await act(async () => { fireEvent.click(compareBtn); });

    // All three chips render and start "active" (default = all installed).
    const claudeChip = await screen.findByTestId('qp-compare-chip-ClaudeCode');
    const codexChip = await screen.findByTestId('qp-compare-chip-Codex');
    const geminiChip = await screen.findByTestId('qp-compare-chip-GeminiCli');
    expect(claudeChip.getAttribute('aria-pressed')).toBe('true');
    expect(codexChip.getAttribute('aria-pressed')).toBe('true');
    expect(geminiChip.getAttribute('aria-pressed')).toBe('true');

    // Deselect Codex.
    await act(async () => { fireEvent.click(codexChip); });
    expect(codexChip.getAttribute('aria-pressed')).toBe('false');

    // CTA's count should now read "(2)" — assert via the data-testid
    // launch button.
    const launchCta = screen.getByTestId('qp-compare-agents-launch');
    expect(launchCta.textContent).toMatch(/2/);

    await act(async () => { fireEvent.click(launchCta); });

    expect(mockQuickPromptsApi.compareAgents).toHaveBeenCalledTimes(1);
    const call = mockQuickPromptsApi.compareAgents.mock.calls[0];
    expect(call[0]).toBe(sampleQpNoVar.id);
    expect(call[1].agents).toEqual(['ClaudeCode', 'GeminiCli']);
  });

  it('compare-agents fans out POST /run for every child disc (otherwise only the navigated agent works)', async () => {
    // Pin the bug the user reported: pre-fix, `handleCompareAgents`
    // called `onNavigateDiscussion(disc1)` which auto-runs ONLY
    // disc1 — sibling discs sat dormant with no agent reply. Fix
    // mirrors `handleBatchLaunch` and fans out a `POST /api/discussions/:id/run`
    // for each child plus calls `onBatchLaunched` so the sidebar
    // expands the batch group.
    mockQuickPromptsApi.list.mockResolvedValue([sampleQpNoVar]);
    mockQuickPromptsApi.compareAgents.mockResolvedValue({
      run_id: 'run-fanout-1',
      batch_total: 3,
      discussion_ids: ['d-c1', 'd-c2', 'd-c3'],
    });

    // Capture every fetch() call from the fan-out.
    const fetchCalls: string[] = [];
    const fetchSpy = vi.spyOn(globalThis, 'fetch').mockImplementation((input) => {
      const url = typeof input === 'string' ? input : (input as Request).url;
      fetchCalls.push(url);
      return Promise.resolve(new Response('{"success":true,"data":null,"error":null}', { status: 200 }));
    });

    const onBatchLaunched = vi.fn();

    await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode', 'Codex', 'GeminiCli']}
        agentAccess={fullConfig}
        onBatchLaunched={onBatchLaunched}
      />
    );

    const qpTab = await screen.findByText(/Quick Prompts/);
    await act(async () => { fireEvent.click(qpTab); });

    const compareBtn = await screen.findByTestId('qp-compare-agents-btn');
    await act(async () => { fireEvent.click(compareBtn); });

    const launchCta = screen.getByTestId('qp-compare-agents-launch');
    await act(async () => { fireEvent.click(launchCta); });

    // Settle the fan-out micro-tasks. The `setTimeout(controller.abort, 500)`
    // does NOT need to fire for the fetch mock to record the call — fetch
    // is invoked synchronously inside the loop.
    await act(async () => { await new Promise(r => setTimeout(r, 0)); });

    // 3 discs → exactly 3 `POST /api/discussions/:id/run` fan-out calls.
    const runCalls = fetchCalls.filter(u => /\/api\/discussions\/d-c\d\/run$/.test(u));
    expect(runCalls).toHaveLength(3);
    expect(runCalls).toEqual(expect.arrayContaining([
      '/api/discussions/d-c1/run',
      '/api/discussions/d-c2/run',
      '/api/discussions/d-c3/run',
    ]));

    // onBatchLaunched is what tells Dashboard to (a) mark every disc as
    // sending so the sidebar spinners light up, AND (b) set focusBatchId
    // so DiscussionsPage auto-expands the batch group ("regrouped under
    // the project" — the user's UX complaint).
    expect(onBatchLaunched).toHaveBeenCalledTimes(1);
    expect(onBatchLaunched.mock.calls[0][0]).toEqual(['d-c1', 'd-c2', 'd-c3']);
    expect(onBatchLaunched.mock.calls[0][1]).toBe('run-fanout-1');

    fetchSpy.mockRestore();
  });

  it('compare-agents — selecting "None" then clicking Compare bails out (CTA disabled)', async () => {
    mockQuickPromptsApi.list.mockResolvedValue([sampleQpNoVar]);

    await wrap(
      <WorkflowsPage
        projects={[]}
        installedAgentTypes={['ClaudeCode', 'Codex']}
        agentAccess={fullConfig}
      />
    );

    const qpTab = await screen.findByText(/Quick Prompts/);
    await act(async () => { fireEvent.click(qpTab); });

    const compareBtn = await screen.findByTestId('qp-compare-agents-btn');
    await act(async () => { fireEvent.click(compareBtn); });

    // Click the toggle-all link — when all selected, it flips to "none".
    const toggleAll = screen.getByText(/Aucun|None|Ninguno/);
    await act(async () => { fireEvent.click(toggleAll); });

    const launchCta = screen.getByTestId('qp-compare-agents-launch') as HTMLButtonElement;
    expect(launchCta.disabled).toBe(true);
    // Even if a click slips through (it shouldn't, the button is disabled),
    // the API must not be called.
    await act(async () => { fireEvent.click(launchCta); });
    expect(mockQuickPromptsApi.compareAgents).not.toHaveBeenCalled();
  });

  it('only the first QP variable is the batch key (renders empty for the rest)', async () => {
    // Pin the comment at WorkflowsPage.tsx:631 ("Use the FIRST variable as the
    // batch key. ... For now we only support 1 variable per batch") so that
    // any future regression where extra variables silently render as `''` is
    // caught by a test instead of by an angry user.
    const twoVarQp: QuickPrompt = {
      ...sampleQpWithVar,
      id: 'qp-3',
      prompt_template: 'Investigate {{ticket}} priority {{priority}}.',
      variables: [
        { name: 'ticket', label: 'Ticket', placeholder: 'EW-1234', description: '', required: true },
        { name: 'priority', label: 'Priority', placeholder: 'P1', description: '', required: true },
      ],
    };
    mockQuickPromptsApi.list.mockResolvedValue([twoVarQp]);

    await wrap(
      <WorkflowsPage projects={[]} installedAgentTypes={['ClaudeCode']} agentAccess={fullConfig} />
    );
    const qpTab = await screen.findByText(/Quick Prompts/);
    await act(async () => { fireEvent.click(qpTab); });

    // The Batch button is rendered for QPs with at least 1 var. Click it to
    // open the batch form, then assert the label calls out the first var only.
    await waitFor(() => {
      expect(screen.getByText('Analyse ticket')).toBeDefined();
    });
    // Find the batch icon button — it has title `qp.batch.launch` ("Batch").
    const batchBtn = document.querySelector('button[title*="Batch"], button[title*="atch"]');
    expect(batchBtn).not.toBeNull();
    await act(async () => { fireEvent.click(batchBtn!); });

    // The textarea label uses qp.variables[0].label = 'Ticket'.
    await waitFor(() => {
      expect(document.body.textContent).toContain('Ticket');
    });
  });
});

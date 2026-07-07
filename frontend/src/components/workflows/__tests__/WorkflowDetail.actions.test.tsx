// WorkflowDetail — actions + run-history coverage.
//
// The collapsed-pipeline view is covered by WorkflowDetail.steps.test.tsx.
// This file targets the rest of the component's behavior:
//   - header actions: edit / refresh / trigger / export wiring + the
//     trigger button's disabled gating (workflow.enabled, triggering),
//   - the trigger-label variants (Manual / Cron / Tracker),
//   - the runs list: empty state, run count, delete-all, collapse toggle,
//   - per-run RunDetail wiring: delete → onDeleteRun, cancel → cancelRun +
//     onRefresh (incl. the catch branch), gate decision → decideRun +
//     onRefresh + onGateDecided,
//   - the synthesized live-run view (effectiveLiveRun from runs[]) +
//     its in-flight cancel button (confirm-gated) hitting cancelRun,
//   - the LiveFinishedBanner status variants (success / waiting / failed),
//   - the "📋 N conversations" batch chip → onNavigateToBatch.
//
// Mocking follows the apiMock factory convention (steps.test.tsx) with
// per-test overrides on the workflows.* methods this component calls.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

const { cancelRun, decideRun, listBatchRunSummaries } = vi.hoisted(() => ({
  cancelRun: vi.fn(),
  decideRun: vi.fn(),
  listBatchRunSummaries: vi.fn(),
}));

vi.mock('../../../lib/api', () =>
  buildApiMock({
    workflows: {
      // decideRun isn't in the default factory shape — wire it here so the
      // gate-decision path resolves. cancelRun + listBatchRunSummaries are
      // in the factory but we want our spies so we can assert calls.
      cancelRun,
      decideRun,
      listBatchRunSummaries,
    },
  }),
);

import { WorkflowDetail, LiveFinishedBanner, type LiveRunState } from '../WorkflowDetail';
import type {
  Workflow,
  WorkflowStep,
  WorkflowRun,
  StepResult,
  WorkflowTrigger,
} from '../../../types/generated';

// ---- fixtures ---------------------------------------------------------

const mkStep = (partial: Partial<WorkflowStep> & { name: string }): WorkflowStep => ({
  step_type: { type: 'Agent' },
  description: null,
  agent: 'ClaudeCode',
  prompt_template: 'do the thing',
  mode: { type: 'Normal' },
  output_format: { type: 'FreeText' },
  mcp_config_ids: [],
  agent_settings: null,
  on_result: [],
  stall_timeout_secs: null,
  retry: null,
  skill_ids: [],
  directive_ids: [],
  profile_ids: [],
  delay_after_secs: null,
  batch_quick_prompt_id: null,
  batch_items_from: null,
  batch_wait_for_completion: null,
  batch_max_items: null,
  batch_workspace_mode: null,
  batch_chain_prompt_ids: [],
  notify_config: null,
  ...partial,
} as unknown as WorkflowStep);

const mkWorkflow = (partial: Partial<Workflow> = {}): Workflow => ({
  id: 'wf-1',
  name: 'Ticket Autopilot',
  project_id: null,
  trigger: { type: 'Manual' } as WorkflowTrigger,
  steps: [
    mkStep({ name: 'analyze', step_type: { type: 'Agent' } }),
    mkStep({ name: 'notify_done', step_type: { type: 'Notify' } }),
  ],
  actions: [],
  safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
  workspace_config: null,
  concurrency_limit: null,
  enabled: true,
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
  ...partial,
} as unknown as Workflow);

const mkStepResult = (partial: Partial<StepResult> & { step_name: string }): StepResult => ({
  status: 'Success',
  output: '',
  tokens_used: 0,
  duration_ms: 0,
  ...partial,
} as unknown as StepResult);

const mkRun = (partial: Partial<WorkflowRun> & { id: string; status: WorkflowRun['status'] }): WorkflowRun => ({
  workflow_id: 'wf-1',
  trigger_context: null,
  step_results: [],
  tokens_used: 0,
  workspace_path: null,
  started_at: new Date('2026-05-29T10:00:00Z').toISOString(),
  finished_at: new Date('2026-05-29T10:01:00Z').toISOString(),
  produced_branches: [],
  ...partial,
} as unknown as WorkflowRun);

// Full-props bag typed against the real component props to dodge tsc's
// narrow-literal rejection on partial spreads.
type Props = React.ComponentProps<typeof WorkflowDetail>;

const baseProps = (): Props => ({
  workflow: mkWorkflow(),
  runs: [],
  liveRun: null,
  onTrigger: vi.fn(),
  onRefresh: vi.fn(),
  onEdit: vi.fn(),
  onDeleteRun: vi.fn(),
  onDeleteAllRuns: vi.fn(),
  triggering: false,
});

const renderDetail = (overrides: Partial<Props> = {}) => {
  const props = { ...baseProps(), ...overrides };
  return { props, ...render(<WorkflowDetail {...props} />) };
};

beforeEach(() => {
  cancelRun.mockReset().mockResolvedValue({ run_cancelled: true, child_discs_cancelled: 0 });
  decideRun.mockReset().mockResolvedValue({ run_id: 'r1', new_status: 'Running' });
  listBatchRunSummaries.mockReset().mockResolvedValue([]);
});

afterEach(() => {
  vi.clearAllMocks();
  vi.unstubAllGlobals();
  cleanup();
});

// ---- header actions ---------------------------------------------------

describe('WorkflowDetail — header actions', () => {
  it('renders the workflow name and the Manual trigger label', () => {
    renderDetail();
    expect(screen.getByText('Ticket Autopilot')).toBeInTheDocument();
    expect(screen.getByText('wf.manual')).toBeInTheDocument();
  });

  it('shows a Cron trigger label with the schedule', () => {
    renderDetail({
      workflow: mkWorkflow({ trigger: { type: 'Cron', schedule: '0 9 * * *' } as unknown as WorkflowTrigger }),
    });
    expect(screen.getByText('Cron: 0 9 * * *')).toBeInTheDocument();
  });

  it('shows a Tracker trigger label with owner/repo', () => {
    renderDetail({
      workflow: mkWorkflow({
        trigger: { type: 'Tracker', source: { owner: 'OrgAlpha', repo: 'svc-beta' } } as unknown as WorkflowTrigger,
      }),
    });
    expect(screen.getByText('Tracker: OrgAlpha/svc-beta')).toBeInTheDocument();
  });

  it('fires onEdit when the edit button is clicked', () => {
    const { props } = renderDetail();
    fireEvent.click(screen.getByText('wf.edit'));
    expect(props.onEdit).toHaveBeenCalledTimes(1);
  });

  it('fires onRefresh when the refresh button is clicked', () => {
    const { props } = renderDetail();
    fireEvent.click(screen.getByText('wf.refresh'));
    expect(props.onRefresh).toHaveBeenCalledTimes(1);
  });

  it('fires onTrigger when the launch button is clicked', () => {
    const { props } = renderDetail();
    fireEvent.click(screen.getByText('wf.launch'));
    expect(props.onTrigger).toHaveBeenCalledTimes(1);
  });

  it('disables the launch button while triggering', () => {
    renderDetail({ triggering: true });
    const launch = screen.getByText('wf.launch').closest('button') as HTMLButtonElement;
    expect(launch.disabled).toBe(true);
  });

  // ── 0.8.11 UX — a disabled workflow must EXPLAIN itself, not sit mute ──
  it('disabled workflow: launch button carries an explanatory tooltip', () => {
    renderDetail({ workflow: mkWorkflow({ enabled: false }) });
    const launch = screen.getByText('wf.launch').closest('button');
    expect(launch).toBeDisabled();
    expect(launch).toHaveAttribute('title', 'wf.launchDisabledHint');
  });

  it('disabled workflow: shows the Désactivé chip and a one-click Activer that calls onToggleEnabled(true)', () => {
    const onToggleEnabled = vi.fn();
    renderDetail({ workflow: mkWorkflow({ enabled: false }), onToggleEnabled });
    expect(screen.getByText(/wf\.disabledChip/)).toBeInTheDocument();
    fireEvent.click(screen.getByText('wf.enableNow'));
    expect(onToggleEnabled).toHaveBeenCalledWith(true);
  });

  it('enabled workflow: no chip, no Activer button', () => {
    renderDetail({ workflow: mkWorkflow({ enabled: true }), onToggleEnabled: vi.fn() });
    expect(screen.queryByText(/wf\.disabledChip/)).toBeNull();
    expect(screen.queryByText('wf.enableNow')).toBeNull();
  });

  it('disables the launch button when the workflow is disabled', () => {
    renderDetail({ workflow: mkWorkflow({ enabled: false }) });
    const launch = screen.getByText('wf.launch').closest('button') as HTMLButtonElement;
    expect(launch.disabled).toBe(true);
  });

  it('renders the export button only when onExport is supplied and wires it', () => {
    const onExport = vi.fn();
    const { rerender } = render(<WorkflowDetail {...baseProps()} />);
    expect(screen.queryByText('wf.export')).toBeNull();
    rerender(<WorkflowDetail {...baseProps()} onExport={onExport} />);
    fireEvent.click(screen.getByText('wf.export'));
    expect(onExport).toHaveBeenCalledTimes(1);
  });
});

// ---- runs list --------------------------------------------------------

describe('WorkflowDetail — runs list', () => {
  it('shows the empty-runs message and no delete-all button when there are no runs', () => {
    renderDetail({ runs: [] });
    expect(screen.getByText('wf.noRuns')).toBeInTheDocument();
    expect(screen.getByText('Runs (0)')).toBeInTheDocument();
    expect(screen.queryByText('wf.deleteAll')).toBeNull();
  });

  it('renders one RunDetail card per run with the run count', () => {
    const runs = [
      mkRun({ id: 'r1', status: 'Success' }),
      mkRun({ id: 'r2', status: 'Failed' }),
    ];
    renderDetail({ runs });
    expect(screen.getByText('Runs (2)')).toBeInTheDocument();
    expect(screen.getByText('Success')).toBeInTheDocument();
    expect(screen.getByText('Failed')).toBeInTheDocument();
  });

  it('fires onDeleteAllRuns from the delete-all button', () => {
    const { props } = renderDetail({ runs: [mkRun({ id: 'r1', status: 'Success' })] });
    fireEvent.click(screen.getByText('wf.deleteAll'));
    expect(props.onDeleteAllRuns).toHaveBeenCalledTimes(1);
  });

  it('collapses the runs list when the chevron toggle is clicked', () => {
    renderDetail({ runs: [mkRun({ id: 'r1', status: 'Success' })] });
    expect(screen.getByText('Success')).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText('Collapse runs'));
    // Collapsed → the run card (and the empty-state) are unmounted.
    expect(screen.queryByText('Success')).toBeNull();
    expect(screen.queryByText('wf.noRuns')).toBeNull();
  });
});

// ---- per-run actions (RunDetail wiring) -------------------------------

describe('WorkflowDetail — per-run actions', () => {
  it('fires onDeleteRun with the run id from a run card delete button', () => {
    const { props } = renderDetail({ runs: [mkRun({ id: 'run-42', status: 'Success' })] });
    // #6 — terminal runs render as a collapsed compact row; expand to reach
    // the full RunDetail (and its delete button).
    fireEvent.click(screen.getByRole('button', { name: /Success/ }));
    fireEvent.click(screen.getByTitle('wf.deleteRun'));
    expect(props.onDeleteRun).toHaveBeenCalledWith('run-42');
  });

  it('cancel on a Running run calls cancelRun then onRefresh', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    const { props } = renderDetail({
      runs: [mkRun({ id: 'run-run', status: 'Running', finished_at: null })],
    });
    // A Running run also synthesizes the live-mini-view, whose own cancel
    // button comes FIRST in DOM. The RunDetail card's cancel (the one that
    // calls onRefresh) is last — click that one.
    const cancelBtns = screen.getAllByTitle('wf.cancelRun');
    fireEvent.click(cancelBtns[cancelBtns.length - 1]);
    await waitFor(() => expect(cancelRun).toHaveBeenCalledWith('wf-1', 'run-run'));
    await waitFor(() => expect(props.onRefresh).toHaveBeenCalled());
  });

  it('swallows a cancelRun rejection without crashing (catch branch)', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    cancelRun.mockRejectedValue(new Error('already finished'));
    const { props } = renderDetail({
      runs: [mkRun({ id: 'run-run', status: 'Running', finished_at: null })],
    });
    const cancelBtns = screen.getAllByTitle('wf.cancelRun');
    fireEvent.click(cancelBtns[cancelBtns.length - 1]);
    await waitFor(() => expect(cancelRun).toHaveBeenCalled());
    // onRefresh is NOT called on the failure path (catch swallows it).
    expect(props.onRefresh).not.toHaveBeenCalled();
  });

  it('gate decision calls decideRun, onRefresh, and onGateDecided', async () => {
    const onGateDecided = vi.fn();
    const gateRun = mkRun({
      id: 'gate-run',
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkStepResult({
          step_name: 'plan_gate',
          status: 'WaitingApproval',
          step_kind: 'Gate',
          output: 'Approve to continue?',
          started_at: new Date('2026-05-29T10:00:30Z').toISOString(),
        }),
      ],
    });
    const { props } = renderDetail({
      workflow: mkWorkflow({ steps: [mkStep({ name: 'plan_gate', step_type: { type: 'Gate' } })] }),
      runs: [gateRun],
      onGateDecided,
    });
    fireEvent.click(screen.getByText('wf.gate.approve'));
    await waitFor(() =>
      expect(decideRun).toHaveBeenCalledWith('wf-1', 'gate-run', { decision: 'approve', comment: null }),
    );
    await waitFor(() => expect(props.onRefresh).toHaveBeenCalled());
    await waitFor(() => expect(onGateDecided).toHaveBeenCalled());
  });
});

// ---- 0.8.11 — per-step model tier badge -----------------------------------
describe('WorkflowDetail — step model tier badge', () => {
  it('shows a tier badge for a non-default (reasoning) Agent step', () => {
    renderDetail({
      workflow: mkWorkflow({
        steps: [mkStep({ name: 'reason', step_type: { type: 'Agent' },
          agent_settings: { tier: 'reasoning' } })],
      }),
    });
    expect(screen.getAllByTitle('disc.tier.reasoning').length).toBeGreaterThan(0);
  });

  it('shows no tier badge for a default-tier step (default = the norm, no noise)', () => {
    renderDetail({
      workflow: mkWorkflow({
        steps: [mkStep({ name: 'reason', step_type: { type: 'Agent' },
          agent_settings: { tier: 'default' } })],
      }),
    });
    expect(screen.queryByTitle('disc.tier.default')).toBeNull();
    expect(screen.queryByTitle('disc.tier.reasoning')).toBeNull();
  });
});

// ---- synthesized live run (effectiveLiveRun from runs[]) --------------

describe('WorkflowDetail — synthesized live run', () => {
  it('derives a live progress view from a Running run when liveRun is null', () => {
    const run = mkRun({
      id: 'live-run',
      status: 'Running',
      finished_at: null,
      step_results: [
        mkStepResult({ step_name: 'analyze', status: 'Success', duration_ms: 5000 }),
      ],
    });
    renderDetail({ runs: [run], liveRun: null });
    // Live block header + progress (1 of 2 steps completed).
    expect(screen.getByText('wf.running')).toBeInTheDocument();
    expect(screen.getByText('(1/2 steps)')).toBeInTheDocument();
  });

  it('synthesized live run exposes a confirm-gated cancel button hitting cancelRun', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    const run = mkRun({
      id: 'live-run',
      status: 'Running',
      finished_at: null,
      step_results: [mkStepResult({ step_name: 'analyze', status: 'Success', duration_ms: 1000 })],
    });
    renderDetail({ runs: [run], liveRun: null });
    // Both the live-block cancel and the RunDetail cancel carry title
    // wf.cancelRun; the live one is the first in DOM order.
    fireEvent.click(screen.getAllByTitle('wf.cancelRun')[0]);
    await waitFor(() => expect(cancelRun).toHaveBeenCalledWith('wf-1', 'live-run'));
  });

  it('does not cancel when the confirm dialog is dismissed', () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(false));
    const run = mkRun({
      id: 'live-run',
      status: 'Running',
      finished_at: null,
      step_results: [mkStepResult({ step_name: 'analyze', status: 'Success', duration_ms: 1000 })],
    });
    renderDetail({ runs: [run], liveRun: null });
    fireEvent.click(screen.getAllByTitle('wf.cancelRun')[0]);
    expect(cancelRun).not.toHaveBeenCalled();
  });

  it('prefers an explicit unfinished liveRun for the current workflow', () => {
    const liveRun: LiveRunState = {
      workflowId: 'wf-1',
      runId: 'sse-run',
      currentStep: 'analyze',
      stepIndex: 0,
      totalSteps: 2,
      completedSteps: [],
      currentStepText: '',
      startedAt: Date.now(),
      currentStepStartedAt: Date.now(),
      finished: false,
      status: 'Running',
    };
    renderDetail({ liveRun, runs: [] });
    expect(screen.getByText('wf.running')).toBeInTheDocument();
    expect(screen.getByText('(0/2 steps)')).toBeInTheDocument();
  });
});

// ---- LiveFinishedBanner (exported) ------------------------------------

describe('WorkflowDetail — finished banner', () => {
  const t = (key: string, ...args: (string | number)[]) =>
    args.length ? `${key}(${args.join('|')})` : key;

  it('renders a success banner', () => {
    const { container } = render(<LiveFinishedBanner status="Success" stepsExecuted={3} t={t} />);
    expect(container.querySelector('[data-status="success"]')).not.toBeNull();
    expect(screen.getByText('wf.runDone(Success)')).toBeInTheDocument();
  });

  it('renders a waiting banner distinct from failed', () => {
    const { container } = render(<LiveFinishedBanner status="WaitingApproval" stepsExecuted={1} t={t} />);
    expect(container.querySelector('[data-status="waiting"]')).not.toBeNull();
    expect(screen.getByText('wf.runWaiting')).toBeInTheDocument();
  });

  it('renders a failed banner for any non-success/non-waiting status', () => {
    const { container } = render(<LiveFinishedBanner status="Failed" stepsExecuted={2} t={t} />);
    expect(container.querySelector('[data-status="failed"]')).not.toBeNull();
    expect(screen.getByText('wf.runDone(Failed)')).toBeInTheDocument();
  });

  it('mounts the finished banner inside the detail tree when liveRun is finished', () => {
    const liveRun: LiveRunState = {
      workflowId: 'wf-1',
      runId: 'done-run',
      currentStep: null,
      stepIndex: 2,
      totalSteps: 2,
      completedSteps: [
        mkStepResult({ step_name: 'analyze', status: 'Success', duration_ms: 1000 }),
        mkStepResult({ step_name: 'notify_done', status: 'Success', duration_ms: 500 }),
      ],
      currentStepText: '',
      startedAt: Date.now(),
      currentStepStartedAt: null,
      finished: true,
      status: 'Success',
    };
    const { container } = renderDetail({ liveRun, runs: [] });
    expect(container.querySelector('.wf-live-finished[data-status="success"]')).not.toBeNull();
  });
});

// ---- batch chip -------------------------------------------------------

describe('WorkflowDetail — batch conversations chip', () => {
  it('renders the batch chip and routes onNavigateToBatch on click', async () => {
    const onNavigateToBatch = vi.fn();
    listBatchRunSummaries.mockResolvedValue([
      {
        run_id: 'batch-99',
        batch_name: null,
        batch_total: 4,
        status: 'Success',
        quick_prompt_id: null,
        quick_prompt_name: null,
        quick_prompt_icon: null,
        parent_run_id: 'run-parent',
        parent_workflow_id: 'wf-1',
        parent_workflow_name: 'Ticket Autopilot',
      },
    ]);
    renderDetail({
      // hasBatchStep gate: the workflow must carry a BatchQuickPrompt step
      // for listBatchRunSummaries to fire.
      workflow: mkWorkflow({
        steps: [mkStep({ name: 'fanout', step_type: { type: 'BatchQuickPrompt' } })],
      }),
      runs: [mkRun({ id: 'run-parent', status: 'Success' })],
      onNavigateToBatch,
    });
    // #6 — the (terminal) parent run is a collapsed compact row; expand it so
    // the batch chip (rendered alongside the full RunDetail) is present.
    fireEvent.click(await screen.findByRole('button', { name: /Success/ }));
    // No I18nProvider in the test tree → useT() falls back to the default
    // context whose t() returns the raw key (args dropped), so the chip
    // label is the bare key.
    const chip = await screen.findByText('wf.runBatchChip');
    fireEvent.click(chip);
    expect(onNavigateToBatch).toHaveBeenCalledWith('batch-99');
  });
});

// ---- #6 compact rows -------------------------------------------------
describe('WorkflowDetail — compact run rows', () => {
  it('collapses a terminal run and expands its detail on click', () => {
    renderDetail({ runs: [mkRun({ id: 'r1', status: 'Success' })] });
    // Collapsed: the full RunDetail (its delete button) is not mounted.
    expect(screen.queryByTitle('wf.deleteRun')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /Success/ }));
    expect(screen.getByTitle('wf.deleteRun')).toBeInTheDocument();
  });

  it('expands a non-terminal run by default (needs attention)', () => {
    renderDetail({ runs: [mkRun({ id: 'r1', status: 'WaitingApproval', finished_at: null })] });
    const row = screen.getByRole('button', { name: /WaitingApproval/ });
    expect(row).toHaveAttribute('aria-expanded', 'true');
  });

  it('#11 — focusRunId auto-expands the targeted (otherwise-collapsed) run', () => {
    renderDetail({
      runs: [
        mkRun({ id: 'other', status: 'Success' }),
        mkRun({ id: 'target-run', status: 'Success' }),
      ],
      focusRunId: 'target-run',
    });
    // The focused terminal run is force-expanded → its RunDetail (delete
    // button) is mounted even though terminal runs collapse by default.
    expect(screen.getByTitle('wf.deleteRun')).toBeInTheDocument();
  });

  it('groups sub-runs of the same parent tick under a collapsible accordion', () => {
    const { container } = renderDetail({
      runs: [
        mkRun({ id: 's1', status: 'Success', parent_run_id: 'tick-1', parent_workflow_name: 'Cron Parent' }),
        mkRun({ id: 's2', status: 'Failed', parent_run_id: 'tick-1', parent_workflow_name: 'Cron Parent' }),
      ],
    });
    // Group header present; both terminal → collapsed by default → no per-run
    // compact rows rendered yet (queried by their distinctive class).
    const header = screen.getByRole('button', { name: /Cron Parent/ });
    expect(header).toHaveAttribute('aria-expanded', 'false');
    expect(container.querySelectorAll('.wf-run-compact')).toHaveLength(0);
    // Expand the group → both sub-run compact rows appear.
    fireEvent.click(header);
    const rows = container.querySelectorAll('.wf-run-compact');
    expect(rows).toHaveLength(2);
    expect([...rows].map(r => r.getAttribute('data-status')).sort()).toEqual(['Failed', 'Success']);
  });
});

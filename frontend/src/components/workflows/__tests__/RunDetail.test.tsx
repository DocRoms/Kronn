// Tests for RunDetail — the panel that surfaces a workflow run's
// step-by-step results.
//
// Coverage focuses on the two recent UX wins:
//  1. Per-step badges built from the StepResult snapshot (`step_kind`,
//     `step_agent`, `step_api_plugin_slug`, `step_api_endpoint_path`)
//     — editing the workflow afterwards must not corrupt the run row.
//  2. LiveStepStatus — when the run is Running, the in-flight step
//     surfaces a step-type-aware activity hint + ticking elapsed
//     counter instead of the static "running…" placeholder.

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act, fireEvent } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { WorkflowRun, WorkflowStep, StepResult } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());

import { RunDetail, tryParseTriageManifest } from '../RunDetail';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t }),
}));

const mkResult = (over: Partial<StepResult>): StepResult => ({
  step_name: 'main',
  status: 'Success',
  output: 'done',
  tokens_used: 0,
  duration_ms: 600,
  is_rollback: false,
  ...over,
});

const mkRun = (over: Partial<WorkflowRun>): WorkflowRun => ({
  id: 'run-1',
  workflow_id: 'wf-1',
  status: 'Success',
  trigger_context: null,
  step_results: [],
  tokens_used: 0,
  workspace_path: null,
  started_at: '2026-04-26T12:00:00Z',
  finished_at: '2026-04-26T12:00:01Z',
  run_type: 'linear', batch_total: 0, batch_completed: 0, batch_failed: 0,
  batch_name: null, parent_run_id: null, state: {}, produced_branches: [],
  ...over,
});

const mkStep = (over: Partial<WorkflowStep>): WorkflowStep => ({
  name: 'main',
  step_type: { type: 'Agent' },
  description: null,
  agent: 'ClaudeCode',
  prompt_template: '',
  mode: { type: 'Normal' },
  output_format: { type: 'FreeText' },
  mcp_config_ids: [],
  agent_settings: null,
  on_result: [],
  stall_timeout_secs: null,
  retry: null,
  delay_after_secs: null,
  skill_ids: [],
  profile_ids: [],
  directive_ids: [],
  batch_quick_prompt_id: null,
  batch_items_from: null,
  batch_wait_for_completion: null,
  batch_max_items: null,
  batch_workspace_mode: null,
  batch_chain_prompt_ids: [],
  notify_config: null,
  api_plugin_slug: null,
  api_config_id: null,
  api_endpoint_path: null,
  api_method: null,
  api_query: null,
  api_headers: null,
  api_body: null,
  api_extract: null,
  api_pagination: null,
  api_timeout_ms: null,
  api_max_retries: null,
  api_output_var: null,
  gate_message: null,
  gate_request_changes_target: null,
  gate_notify_url: null,
  exec_command: null,
  exec_args: [],
  exec_timeout_secs: null,
  ...over,
});

describe('RunDetail — step_kind snapshot badges (run history honesty)', () => {
  beforeEach(() => {
    // Some tests render a Running run; freeze Date.now so the elapsed
    // counter is deterministic.
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-26T12:00:30Z'));
  });

  it('renders the API plugin + endpoint subtitle for an ApiCall step result', () => {
    const run = mkRun({
      step_results: [
        mkResult({
          step_name: 'fetch',
          step_kind: 'ApiCall',
          step_api_plugin_slug: 'mcp-github',
          step_api_endpoint_path: '/repos/anthropics/cookbook/issues',
        }),
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.getByText('API')).toBeInTheDocument();
    expect(screen.getByText(/mcp-github · \/repos\/anthropics\/cookbook\/issues/)).toBeInTheDocument();
  });

  it('shows the resolved model/tier badge on an Agent step (step_model)', () => {
    const run = mkRun({
      step_results: [mkResult({ step_kind: 'Agent', step_agent: 'ClaudeCode', step_model: 'sonnet · reasoning' })],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.getByText('sonnet · reasoning')).toBeInTheDocument();
  });

  it('renders the agent label for a snapshotted Agent step', () => {
    const run = mkRun({
      step_results: [mkResult({ step_kind: 'Agent', step_agent: 'Codex' })],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.getByText('Codex')).toBeInTheDocument();
  });

  it('renders a NOTIFY badge for snapshotted Notify steps', () => {
    const run = mkRun({
      step_results: [mkResult({ step_kind: 'Notify' })],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.getByText('NOTIFY')).toBeInTheDocument();
  });

  it('surfaces a foreach fan-out: chip on the row, per-task table when expanded', async () => {
    const envelope = JSON.stringify({
      data: {
        mode: 'foreach', total: 3, succeeded: 2, failed: 1,
        items: [
          { item: 0, id: 'scss-tokens', status: 'MechanicalApplied', child_run_id: null, files: 1 },
          { item: 1, id: 'brand-context', status: 'Success', child_run_id: 'aaaabbbb-1111' },
          { item: 2, id: 'twig-loader', status: 'Failed', child_run_id: 'ccccdddd-2222' },
        ],
      },
      status: 'OK', summary: 'partial',
    });
    const run = mkRun({
      step_results: [mkResult({
        step_name: 'feasibility_impl',
        step_kind: 'SubWorkflow',
        output: `---STEP_OUTPUT---\n${envelope}\n---END_STEP_OUTPUT---`,
      })],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    // PARTIAL surfacing: the engine keeps the step Success, the chip warns.
    expect(screen.getByTestId('wf-foreach-chip').textContent).toContain('2/3');
    expect(screen.getByTestId('wf-foreach-chip').textContent).toContain('1 wf.foreachFailedChip');
    // expand → per-task table with ids, statuses (0 tk hint) and child runs
    await act(async () => { screen.getByText('feasibility_impl').click(); });
    const table = screen.getByTestId('wf-foreach-table');
    expect(table).toBeInTheDocument();
    expect(screen.getByText('scss-tokens')).toBeInTheDocument();
    expect(screen.getByText('MechanicalApplied · 0 tk')).toBeInTheDocument();
    expect(screen.getByText('Failed')).toBeInTheDocument();
    expect(screen.getByText('aaaabbbb…')).toBeInTheDocument();
  });

  it('per-task child_run_id links to the child sub-workflow when nav + steps provided', async () => {
    const envelope = JSON.stringify({
      data: { mode: 'foreach', total: 1, succeeded: 1, failed: 0,
        items: [{ item: 0, id: 'brand-enum', status: 'Success', child_run_id: 'aaaabbbb-1111' }] },
      status: 'OK', summary: 'ok',
    });
    const run = mkRun({
      step_results: [mkResult({ step_name: 'feasibility_impl', step_kind: 'SubWorkflow',
        output: `---STEP_OUTPUT---\n${envelope}\n---END_STEP_OUTPUT---` })],
    });
    const steps = [mkStep({ name: 'feasibility_impl', step_type: { type: 'SubWorkflow' }, sub_workflow_id: 'child-wf-99' })];
    const onNav = vi.fn();
    render(<RunDetail run={run} workflowSteps={steps} onNavigateToWorkflow={onNav} onDelete={() => {}} />);
    await act(async () => { screen.getByText('feasibility_impl').click(); });
    const link = screen.getByTitle('wf.openSubRun');
    link.click();
    expect(onNav).toHaveBeenCalledWith('child-wf-99');
  });

  it('#11 — child_run_id link drills to the EXACT child run when onNavigateToRun is provided', async () => {
    const envelope = JSON.stringify({
      data: { mode: 'foreach', total: 1, succeeded: 1, failed: 0,
        items: [{ item: 0, id: 'brand-enum', status: 'Success', child_run_id: 'aaaabbbb-1111' }] },
      status: 'OK', summary: 'ok',
    });
    const run = mkRun({
      step_results: [mkResult({ step_name: 'feasibility_impl', step_kind: 'SubWorkflow',
        output: `---STEP_OUTPUT---\n${envelope}\n---END_STEP_OUTPUT---` })],
    });
    const steps = [mkStep({ name: 'feasibility_impl', step_type: { type: 'SubWorkflow' }, sub_workflow_id: 'child-wf-99' })];
    const onNavRun = vi.fn();
    const onNavWf = vi.fn();
    render(<RunDetail run={run} workflowSteps={steps} onNavigateToWorkflow={onNavWf} onNavigateToRun={onNavRun} onDelete={() => {}} />);
    await act(async () => { screen.getByText('feasibility_impl').click(); });
    screen.getByTitle('wf.openSubRun').click();
    // onNavigateToRun wins over onNavigateToWorkflow and carries the run id.
    expect(onNavRun).toHaveBeenCalledWith('child-wf-99', 'aaaabbbb-1111');
    expect(onNavWf).not.toHaveBeenCalled();
  });

  it('shows no foreach chip for a single-child SubWorkflow envelope', () => {
    const run = mkRun({
      step_results: [mkResult({
        step_kind: 'SubWorkflow',
        output: `---STEP_OUTPUT---\n${JSON.stringify({ data: { child_run_id: 'x', child_status: 'Success' }, status: 'OK', summary: 's' })}\n---END_STEP_OUTPUT---`,
      })],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.queryByTestId('wf-foreach-chip')).not.toBeInTheDocument();
  });

  it('renders nothing extra for legacy rows (step_kind absent) — no crash, graceful fallback', () => {
    const run = mkRun({
      step_results: [mkResult({ step_kind: null })],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.getByText('main')).toBeInTheDocument();
    // No badge, no agent label — none of the snapshot fields are set,
    // so the row degrades gracefully to "step name + duration only".
    expect(screen.queryByText('API')).not.toBeInTheDocument();
    expect(screen.queryByText('NOTIFY')).not.toBeInTheDocument();
  });
});

describe('RunDetail — LiveStepStatus (in-flight step UX)', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-26T12:00:30Z'));
  });

  it('shows a per-type activity hint + elapsed counter for the in-flight step', () => {
    // Run started at 12:00:00, no completed steps yet → in-flight step
    // started at run.started_at. Now is 12:00:30 → elapsed = 30s.
    const run = mkRun({
      status: 'Running',
      step_results: [],
      finished_at: null,
    });
    render(
      <RunDetail
        run={run}
        workflowSteps={[mkStep({ name: 'fetch', step_type: { type: 'ApiCall' } })]}
        onDelete={() => {}}
      />,
    );
    // The step-type-aware activity hint replaces the generic "running…".
    expect(screen.getByText(/wf\.liveStep\.api/)).toBeInTheDocument();
    // Elapsed counter shows seconds. Exact value depends on the started_at
    // → now diff (30s here).
    expect(screen.getByText('30s')).toBeInTheDocument();
  });

  it('displays the agent activity hint for an in-flight Agent step', () => {
    const run = mkRun({
      status: 'Running',
      step_results: [],
      finished_at: null,
    });
    render(
      <RunDetail
        run={run}
        workflowSteps={[mkStep({ name: 'analyze', step_type: { type: 'Agent' } })]}
        onDelete={() => {}}
      />,
    );
    expect(screen.getByText(/wf\.liveStep\.agent/)).toBeInTheDocument();
  });

  it('estimates step start = run.started_at + sum(completed durations)', () => {
    // Run started at 12:00:00, step 0 took 10s, step 1 is in-flight.
    // Now 12:00:30 → step 1 elapsed ≈ 20s.
    const run = mkRun({
      status: 'Running',
      step_results: [mkResult({ step_name: 'a', duration_ms: 10000 })],
      finished_at: null,
    });
    render(
      <RunDetail
        run={run}
        workflowSteps={[
          mkStep({ name: 'a', step_type: { type: 'Agent' } }),
          mkStep({ name: 'b', step_type: { type: 'Agent' } }),
        ]}
        onDelete={() => {}}
      />,
    );
    expect(screen.getByText('20s')).toBeInTheDocument();
  });
});

describe('RunDetail — Gate decision panel (0.7.0 Phase 4 — human-in-the-loop)', () => {
  beforeEach(() => {
    // Sibling describes leave fake timers installed; the gate tests
    // rely on synchronous state updates and don't need fakes.
    vi.useRealTimers();
  });

  it('renders the gate panel with the rendered message when run is WaitingApproval', () => {
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({
          step_name: 'pause_pre_merge',
          step_kind: 'Gate',
          status: 'WaitingApproval',
          output: 'Validate the PR for ticket EW-42?',
        }),
      ],
    });
    const { container } = render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    // Title and message visible (the message also appears in the step
    // list preview, hence we scope the assertion to the panel itself).
    expect(screen.getByText(/wf\.gate\.title/)).toBeInTheDocument();
    const panel = container.querySelector('.wf-gate-panel');
    expect(panel).not.toBeNull();
    expect(panel!.textContent).toContain('Validate the PR for ticket EW-42?');
    // All three buttons.
    expect(screen.getByText(/wf\.gate\.approve/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.gate\.requestChanges/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.gate\.reject/)).toBeInTheDocument();
  });

  it('does not render the panel when no onDecide handler is provided', () => {
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({
          step_name: 'pause_pre_merge',
          step_kind: 'Gate',
          status: 'WaitingApproval',
          output: 'Approve?',
        }),
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.queryByText(/wf\.gate\.title/)).not.toBeInTheDocument();
  });

  it('does not render the panel when status is WaitingApproval but trailing step is not Gate', () => {
    // Defensive: should never happen in practice (only Gate steps emit
    // WaitingApproval), but the render path must guard against it
    // rather than try to read a message off a non-Gate StepResult.
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({
          step_name: 'agent_step',
          step_kind: 'Agent',
          status: 'WaitingApproval',
          output: 'mismatched state',
        }),
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    expect(screen.queryByText(/wf\.gate\.title/)).not.toBeInTheDocument();
  });

  it('calls onDecide with approve + null comment when Approve is clicked', async () => {
    const onDecide = vi.fn().mockResolvedValue(undefined);
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'gate', step_kind: 'Gate', status: 'WaitingApproval', output: 'OK?' }),
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} onDecide={onDecide} />);
    const approveBtn = screen.getByText(/wf\.gate\.approve/).closest('button')!;
    await act(async () => {
      approveBtn.click();
    });
    expect(onDecide).toHaveBeenCalledWith({ decision: 'approve', comment: null });
  });

  it('blocks request_changes when comment is empty (UX guard before API)', async () => {
    const onDecide = vi.fn().mockResolvedValue(undefined);
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'gate', step_kind: 'Gate', status: 'WaitingApproval', output: 'OK?' }),
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} onDecide={onDecide} />);
    const changesBtn = screen.getByText(/wf\.gate\.requestChanges/).closest('button')!;
    await act(async () => {
      changesBtn.click();
    });
    expect(onDecide).not.toHaveBeenCalled();
    // Inline error visible to operator after the React state flush.
    expect(screen.getByText(/wf\.gate\.commentRequired/)).toBeInTheDocument();
  });

  // 0.8.3 — Feasibility-Gated triage manifest rendering. When the
  // review_triage Gate's message wraps a triage manifest, the raw
  // JSON dump is replaced by a structured panel with one collapsible
  // section per category. Non-triage Gates fall back to the raw text.
  it('renders the structured triage panel when message wraps a manifest', () => {
    const manifest = {
      clear: [
        { id: 'brand-enum', what: 'Create Brand backed enum', where: 'src/Enum/Brand.php' },
      ],
      decided: [
        {
          id: 'brand-context-impl',
          what: 'How to inject brand context',
          chosen: 'Stateful service',
          why: 'Matches existing LocaleService pattern',
          options_considered: ['Request attribute', 'Compiler pass'],
        },
      ],
      mocked: [
        {
          id: 'adobe-dtm',
          what: 'Adobe DTM URLs',
          placeholder: 'legacy values lifted from front_africanews',
          strategy: 'Replace with Data team confirmation',
          revisit_when: 'Data team responds',
        },
      ],
      blocked: [
        {
          id: 'visitor-ns',
          what: 'Africanews visitorNamespace',
          why: 'Not present in legacy codebase',
          needed_from: 'Data team',
          workaround: 'Empty-string KRONN-MOCKED placeholder',
        },
      ],
      files_touched: ['src/Enum/Brand.php', 'src/Listener/BrandListener.php'],
    };
    const message = `Triage manifest produced. Review the four categories below before approving:\n\n${JSON.stringify(manifest)}\n\n- Approve\n- Request changes`;
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'review_triage', step_kind: 'Gate', status: 'WaitingApproval', output: message }),
      ],
    });
    const { container } = render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    // Structured panel mounts (vs. .wf-gate-message that holds raw text on non-triage gates)
    expect(container.querySelector('.wf-triage-manifest')).not.toBeNull();
    // Each of the four section headers is rendered with its count badge
    expect(screen.getByText(/wf\.gate\.triage\.clear/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.gate\.triage\.decided/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.gate\.triage\.mocked/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.gate\.triage\.blocked/)).toBeInTheDocument();
    // Per-entry surface: id + chosen line for the `decided` entry
    expect(screen.getByText('brand-context-impl')).toBeInTheDocument();
    expect(screen.getByText('Stateful service')).toBeInTheDocument();
    // Per-entry surface for `blocked`: needed_from is the killer field the
    // user reads to know which team to ping
    expect(screen.getByText('Data team')).toBeInTheDocument();
    // files_touched collapsible footer summarizes the count
    expect(screen.getByText(/wf\.gate\.triage\.filesTouched:2/)).toBeInTheDocument();
    // Decision buttons still wired below the manifest panel
    expect(screen.getByText(/wf\.gate\.approve/)).toBeInTheDocument();
  });

  it('falls back to the raw message when the gate is not a triage gate', () => {
    // A plain "approve to deploy?" Gate with no embedded JSON must NOT
    // trigger the structured panel — defensive against false positives.
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({
          step_name: 'pause_pre_merge',
          step_kind: 'Gate',
          status: 'WaitingApproval',
          output: 'Approve the merge to main?',
        }),
      ],
    });
    const { container } = render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    expect(container.querySelector('.wf-triage-manifest')).toBeNull();
    expect(container.querySelector('.wf-gate-message')!.textContent)
      .toContain('Approve the merge to main?');
  });

  // ── TriageManifestPanel edge cases (0.8.3, TD-270) ─────────────────
  //
  // Beyond the "happy path" + "fallback to raw text" tests above, the
  // panel has its own rendering branches: empty categories, missing
  // files_touched, options_considered absent, and the toggle behavior
  // (sections collapse + expand). These tests pin them so a future
  // refactor of the panel doesn't silently break the empty-state UX.

  const buildTriageMsg = (manifest: object) =>
    `Review:\n\n${JSON.stringify(manifest)}\n\n- Approve`;

  it('renders all 4 section headers even when categories are empty', () => {
    // An empty manifest is degenerate but valid — the agent classified
    // everything as clear and one category happened to be empty. The
    // panel must still render the headers with count=0 (silently
    // hiding them would mask "the agent found nothing here", which
    // IS the operator's signal that the run is over-confident).
    const msg = buildTriageMsg({ clear: [], decided: [], mocked: [], blocked: [] });
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'review_triage', step_kind: 'Gate', status: 'WaitingApproval', output: msg }),
      ],
    });
    const { container } = render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    expect(container.querySelector('.wf-triage-manifest')).not.toBeNull();
    // All 4 section headers present
    expect(screen.getByText(/wf\.gate\.triage\.clear/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.gate\.triage\.decided/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.gate\.triage\.mocked/)).toBeInTheDocument();
    expect(screen.getByText(/wf\.gate\.triage\.blocked/)).toBeInTheDocument();
    // 4 count badges showing 0
    const counts = container.querySelectorAll('.wf-triage-cat-count');
    expect(counts).toHaveLength(4);
    counts.forEach(el => expect(el.textContent).toBe('0'));
  });

  it('omits the files_touched footer when manifest has no files', () => {
    // files_touched is optional; when absent (or empty) the footer
    // must be hidden so it doesn't bait the operator into expanding
    // an empty list.
    const msg = buildTriageMsg({
      clear: [{ id: 'a', what: 'A' }], decided: [], mocked: [], blocked: [],
    });
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'review_triage', step_kind: 'Gate', status: 'WaitingApproval', output: msg }),
      ],
    });
    const { container } = render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    expect(container.querySelector('.wf-triage-files')).toBeNull();
    expect(screen.queryByText(/wf\.gate\.triage\.filesTouched/)).not.toBeInTheDocument();
  });

  it('renders the files_touched footer with count when present', () => {
    const msg = buildTriageMsg({
      clear: [], decided: [], mocked: [], blocked: [],
      files_touched: ['src/a.rs', 'src/b.rs', 'src/c.rs'],
    });
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'review_triage', step_kind: 'Gate', status: 'WaitingApproval', output: msg }),
      ],
    });
    const { container } = render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    expect(container.querySelector('.wf-triage-files')).not.toBeNull();
    // Count interpolated via i18n placeholder
    expect(screen.getByText(/wf\.gate\.triage\.filesTouched:3/)).toBeInTheDocument();
  });

  it('omits options_considered sub-collapsible when the decided entry has none', () => {
    // `options_considered` is optional — a decided entry without it
    // is fine (some decisions have no rejected alternatives worth
    // listing). The collapsible must NOT render in that case so
    // the empty UI doesn't bait the operator.
    const msg = buildTriageMsg({
      clear: [], mocked: [], blocked: [],
      decided: [{
        id: 'no-opts', what: 'simple decision', chosen: 'X', why: 'Y',
        // no options_considered
      }],
    });
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'review_triage', step_kind: 'Gate', status: 'WaitingApproval', output: msg }),
      ],
    });
    const { container } = render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    expect(container.querySelector('.wf-triage-entry-considered')).toBeNull();
    expect(screen.queryByText(/wf\.gate\.triage\.optionsConsidered/)).not.toBeInTheDocument();
  });

  it('toggles a section open ↔ closed when the summary is clicked', async () => {
    // Sections start with decided/mocked/blocked OPEN (most decisions
    // to review) and clear CLOSED (mechanical, less urgent). Clicking
    // the summary must flip the state — a regression here would
    // freeze the panel in its initial layout.
    const msg = buildTriageMsg({
      clear: [{ id: 'c1', what: 'clear entry' }],
      decided: [], mocked: [], blocked: [],
    });
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'review_triage', step_kind: 'Gate', status: 'WaitingApproval', output: msg }),
      ],
    });
    const { container } = render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    const clearSection = container.querySelector('.wf-triage-section[data-cat="clear"]') as HTMLDetailsElement;
    expect(clearSection).not.toBeNull();
    // Initial: clear is closed (mechanical, lower priority)
    expect(clearSection.open).toBe(false);
    // Click summary → opens
    const summary = clearSection.querySelector('summary')!;
    await act(async () => {
      summary.click();
    });
    expect(clearSection.open).toBe(true);
    // Click again → closes
    await act(async () => {
      summary.click();
    });
    expect(clearSection.open).toBe(false);
  });
});

describe('tryParseTriageManifest — JSON extraction from Gate message (0.8.3)', () => {
  // The parser scans the Gate message for the first `{`, brace-counts
  // to the matching `}`, parses, then validates the manifest shape.
  // These tests lock in the edge cases the brace-counter has to handle
  // — every one of them was a real risk of false-negative (manifest
  // present but parser bails) or false-positive (random JSON
  // accidentally accepted as a triage manifest).

  const validManifest = {
    clear: [{ id: 'a', what: 'A' }],
    decided: [{ id: 'b', what: 'B', chosen: 'X', why: 'Y' }],
    mocked: [],
    blocked: [],
  };

  it('returns null on empty message (defensive)', () => {
    expect(tryParseTriageManifest('')).toBeNull();
  });

  it('returns null when there is no JSON object at all', () => {
    expect(tryParseTriageManifest('Approve the merge to main?')).toBeNull();
  });

  it('returns null on malformed JSON (unclosed brace)', () => {
    // Brace-counter walks to end of string without depth=0 — must
    // return null rather than throw.
    const msg = 'prose {"clear": [{ "id": "a", "what": "A" }';
    expect(tryParseTriageManifest(msg)).toBeNull();
  });

  it('returns null when JSON is valid but not a manifest shape', () => {
    // {"foo": 1} parses fine but has no `clear/decided/mocked/blocked`
    // arrays — defensive guard against random JSON in a non-triage Gate
    // message accidentally matching the panel.
    expect(tryParseTriageManifest('Approve? {"foo": 1}')).toBeNull();
  });

  it('returns null when ANY of the 4 categories is not an array', () => {
    // All 4 array-checks must pass — missing or wrong-typed category
    // bails. Without this guard we would render `.map()` on undefined
    // and crash the panel.
    const m = { ...validManifest, blocked: 'not an array' };
    expect(tryParseTriageManifest(`x ${JSON.stringify(m)}`)).toBeNull();
  });

  it('returns null when ONE of the 4 categories is missing entirely', () => {
    const m: Record<string, unknown> = { ...validManifest };
    delete m.blocked;
    expect(tryParseTriageManifest(`x ${JSON.stringify(m)}`)).toBeNull();
  });

  it('parses a manifest preceded by prose preamble (the real shape)', () => {
    // This is what the runner actually emits — prose + JSON on its
    // own paragraph. The parser must skip the prose and pick the
    // first `{`.
    const msg = `Triage manifest produced. Review the categories:\n\n${JSON.stringify(validManifest)}\n\n- Approve...`;
    const m = tryParseTriageManifest(msg);
    expect(m).not.toBeNull();
    expect(m!.decided[0].id).toBe('b');
  });

  it('handles braces inside string values without mis-counting', () => {
    // The brace-counter must respect strings: a `{` or `}` inside a
    // quoted value must NOT change depth, otherwise the parser stops
    // at the wrong character and trims valid JSON.
    const m = {
      clear: [{ id: 'a', what: 'String with {} braces inside' }],
      decided: [], mocked: [], blocked: [],
    };
    const parsed = tryParseTriageManifest(`x ${JSON.stringify(m)} y`);
    expect(parsed).not.toBeNull();
    expect(parsed!.clear[0].what).toContain('{}');
  });

  it('handles escaped quotes inside strings without mis-counting', () => {
    // `\"` inside a string must not terminate the string for the
    // brace-counter's string-mode tracking. JSON.stringify produces
    // this for any value containing a `"`.
    const m = {
      clear: [{ id: 'a', what: 'value with "embedded" quotes' }],
      decided: [], mocked: [], blocked: [],
    };
    const parsed = tryParseTriageManifest(`prose ${JSON.stringify(m)}`);
    expect(parsed).not.toBeNull();
    expect(parsed!.clear[0].what).toContain('embedded');
  });

  it('handles deeply nested objects inside entries', () => {
    // `decided` entries carry `options_considered` (array of strings)
    // but downstream features may push richer nested structures.
    // Parser must brace-count through all depths.
    const m = {
      clear: [], mocked: [], blocked: [],
      decided: [{
        id: 'x', what: 'y', chosen: 'z', why: 'w',
        options_considered: ['opt1', 'opt2', 'opt3'],
      }],
    };
    const parsed = tryParseTriageManifest(`x ${JSON.stringify(m)}`);
    expect(parsed).not.toBeNull();
    expect(parsed!.decided[0].options_considered).toHaveLength(3);
  });

  it('returns null when all categories are present but empty arrays — wait, this is valid', () => {
    // Empty manifest is a degenerate but VALID shape (every entry
    // classifies as clear and the agent emitted nothing).
    // Important: parser accepts; panel handles count=0 gracefully.
    const m = { clear: [], decided: [], mocked: [], blocked: [] };
    expect(tryParseTriageManifest(`x ${JSON.stringify(m)}`)).not.toBeNull();
  });
});

describe('RunDetail — B5 (0.6.0 UX pass) WaitingApproval badge + PausedSince', () => {
  beforeEach(() => {
    // Freeze the clock so PausedSince is deterministic. Run started at
    // 12:00:00 + 1 step took 60s (gate enters at 12:01:00). "Now" is
    // 12:31:00 → expected pause = 30 min.
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-04-26T12:31:00Z'));
  });

  it('renders the À VALIDER badge instead of generic status when WaitingApproval', () => {
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        mkResult({ step_name: 'prep', step_kind: 'Agent', duration_ms: 60000 }),
        mkResult({ step_name: 'gate', step_kind: 'Gate', status: 'WaitingApproval', output: 'OK?' }),
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    // The badge replaces the raw status text — explicit translation key.
    expect(screen.getByText(/wf\.runStatusToReview/)).toBeInTheDocument();
  });

  it('shows "paused for 30 min" counter using completed step durations', () => {
    const run = mkRun({
      status: 'WaitingApproval',
      finished_at: null,
      step_results: [
        // Step duration: 1 minute. Pause start = run.started_at + 60s.
        mkResult({ step_name: 'prep', step_kind: 'Agent', duration_ms: 60000 }),
        mkResult({ step_name: 'gate', step_kind: 'Gate', status: 'WaitingApproval', duration_ms: 0, output: 'OK?' }),
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} onDecide={() => {}} />);
    // Translation key carries `{0}` — i18n stub `t(key, ...args)` renders
    // it as `wf.pausedMinutes:30`. Substring match is enough.
    expect(screen.getByText(/wf\.pausedMinutes:30/)).toBeInTheDocument();
    // The "0 token" badge sits next to the counter — argument commercial
    // qu'Antony a flaggé comme killer feature à mettre en avant.
    expect(screen.getByText(/wf\.pausedZeroTokens/)).toBeInTheDocument();
  });

  it('does NOT show the pause counter when run is not WaitingApproval', () => {
    const run = mkRun({
      status: 'Success',
      step_results: [mkResult({ step_name: 'main', step_kind: 'Agent' })],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.queryByText(/wf\.pausedMinutes/)).not.toBeInTheDocument();
    expect(screen.queryByText(/wf\.runStatusToReview/)).not.toBeInTheDocument();
  });
});

// ─── ProducedBranches panel (0.7.0 — surfaces commits the agent committed
//    in its worktree but couldn't push) ────────────────────────────────
describe('RunDetail — ProducedBranches panel', () => {
  it('renders when the run carries non-empty produced_branches', () => {
    const run = mkRun({
      status: 'Success',
      produced_branches: [
        {
          branch_name: 'kronn/Autobot/abcdef12',
          head_sha: 'b71d816b77f04670d6e07a937530e20d83f76010',
          ahead: 1,
          pushed_upstream: false,
        },
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    // The localized title carries the count; we use the i18n stub which
    // renders `wf.produced.title:1`.
    expect(screen.getByText(/wf\.produced\.title/)).toBeInTheDocument();
    expect(screen.getByText('kronn/Autobot/abcdef12')).toBeInTheDocument();
    // Short SHA shown (8 chars).
    expect(screen.getByText('b71d816b')).toBeInTheDocument();
    // "Local only" pill for unpushed branches.
    expect(screen.getByText(/wf\.produced\.localOnly/)).toBeInTheDocument();
  });

  it('shows the "pushed" pill when pushed_upstream=true', () => {
    const run = mkRun({
      status: 'Success',
      produced_branches: [
        {
          branch_name: 'kronn/Autobot/pushed',
          head_sha: 'aaaaaaaa11112222333344445555666677778888',
          ahead: 0,
          pushed_upstream: true,
        },
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.getByText(/wf\.produced\.pushed/)).toBeInTheDocument();
    expect(screen.queryByText(/wf\.produced\.localOnly/)).not.toBeInTheDocument();
  });

  it('does NOT render the panel when produced_branches is empty/undefined', () => {
    const runEmpty = mkRun({ status: 'Success', produced_branches: [] });
    const { rerender } = render(<RunDetail run={runEmpty} onDelete={() => {}} />);
    expect(screen.queryByText(/wf\.produced\.title/)).not.toBeInTheDocument();
    // Same for missing field (legacy run row).
    rerender(<RunDetail run={mkRun({ status: 'Success' })} onDelete={() => {}} />);
    expect(screen.queryByText(/wf\.produced\.title/)).not.toBeInTheDocument();
  });

  it('renders one row per branch when there are several', () => {
    const run = mkRun({
      status: 'Success',
      produced_branches: [
        { branch_name: 'kronn/A/11111111', head_sha: '1'.repeat(40), ahead: 1, pushed_upstream: false },
        { branch_name: 'kronn/A/22222222', head_sha: '2'.repeat(40), ahead: 2, pushed_upstream: true },
      ],
    });
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.getByText('kronn/A/11111111')).toBeInTheDocument();
    expect(screen.getByText('kronn/A/22222222')).toBeInTheDocument();
  });
});

describe('RunDetail — sub-workflow provenance', () => {
  it('shows a clickable provenance pill resolving to the parent workflow', () => {
    const onNav = vi.fn();
    const run = mkRun({
      run_type: 'subworkflow',
      parent_run_id: 'parent-run-9',
      parent_workflow_id: 'cron-wf',
      parent_workflow_name: 'PR Review cron v2',
      parent_run_started_at: '2026-07-06T06:13:00Z',
    });
    render(<RunDetail run={run} onDelete={() => {}} onNavigateToWorkflow={onNav} />);
    const pill = screen.getByText('PR Review cron v2');
    expect(pill).toBeInTheDocument();
    fireEvent.click(pill);
    expect(onNav).toHaveBeenCalledWith('cron-wf');
  });

  it('renders no provenance pill for a top-level run', () => {
    const run = mkRun({ run_type: 'linear' });
    render(<RunDetail run={run} onDelete={() => {}} onNavigateToWorkflow={() => {}} />);
    expect(screen.queryByText(/PR Review cron/)).not.toBeInTheDocument();
  });

  it('shows the pill but disables navigation when parent workflow id is absent', () => {
    const onNav = vi.fn();
    const run = mkRun({
      run_type: 'subworkflow',
      parent_workflow_name: 'Orphan Parent',
      parent_workflow_id: null,
    });
    render(<RunDetail run={run} onDelete={() => {}} onNavigateToWorkflow={onNav} />);
    const pill = screen.getByText('Orphan Parent');
    fireEvent.click(pill);
    expect(onNav).not.toHaveBeenCalled();
  });
});

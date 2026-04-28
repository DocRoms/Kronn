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
import { render, screen, act } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { WorkflowRun, WorkflowStep, StepResult } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());

import { RunDetail } from '../RunDetail';

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

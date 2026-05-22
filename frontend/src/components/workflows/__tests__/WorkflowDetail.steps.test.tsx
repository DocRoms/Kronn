// Collapsed steps pipeline (WorkflowDetail).
//
// The Steps panel used to dump every per-step card (prompt + Test button)
// stacked vertically — heavy and rarely what you want at a glance,
// especially mid-run. It now collapses to a compact pipeline (number +
// kind icon + name) with an agent/deterministic count split, and "Voir en
// détails" reveals the legacy cards. These tests guard:
//   - collapsed by default (no detail cards rendered),
//   - the agent (token) vs deterministic classification drives chip colors,
//   - the toggle expands to the full card list.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

vi.mock('../../../lib/api', () => buildApiMock());

import { WorkflowDetail } from '../WorkflowDetail';
import type { Workflow, WorkflowStep } from '../../../types/generated';

// Minimal step factory — spreads the defaults the renderer dereferences so
// each test only states the discriminating fields (name + step_type).
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

const mkWorkflow = (steps: WorkflowStep[]): Workflow => ({
  id: 'wf-1',
  name: 'Ticket Autopilot',
  project_id: null,
  trigger: { type: 'Manual' },
  steps,
  actions: [],
  safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
  workspace_config: null,
  concurrency_limit: null,
  enabled: true,
  created_at: new Date().toISOString(),
  updated_at: new Date().toISOString(),
} as unknown as Workflow);

// 1 agent (tokens) + 4 deterministic (api / gate / exec / notify).
const mixedSteps = [
  mkStep({ name: 'analyze', step_type: { type: 'Agent' } }),
  mkStep({ name: 'fetch_issue', step_type: { type: 'ApiCall' } }),
  mkStep({ name: 'plan_gate', step_type: { type: 'Gate' } }),
  mkStep({ name: 'run_tests', step_type: { type: 'Exec' } }),
  mkStep({ name: 'notify_done', step_type: { type: 'Notify' } }),
];

const renderDetail = (steps: WorkflowStep[]) =>
  render(
    <WorkflowDetail
      workflow={mkWorkflow(steps)}
      runs={[]}
      liveRun={null}
      onTrigger={() => {}}
      onRefresh={() => {}}
      onEdit={() => {}}
      onDeleteRun={() => {}}
      onDeleteAllRuns={() => {}}
      triggering={false}
    />
  );

describe('WorkflowDetail — collapsed steps pipeline', () => {
  it('renders collapsed by default — pipeline shown, full detail cards hidden', () => {
    const { container } = renderDetail(mixedSteps);
    expect(screen.getByTestId('wf-steps-section')).toBeInTheDocument();
    // Compact pipeline shows one chip per step, with the step name.
    const chips = container.querySelectorAll('.wf-pipe-chip');
    expect(chips.length).toBe(5);
    expect(screen.getByText('analyze')).toBeInTheDocument();
    expect(screen.getByText('notify_done')).toBeInTheDocument();
    // The heavy per-step cards are NOT mounted until expanded.
    expect(screen.queryByTestId('wf-steps-detail')).toBeNull();
    expect(container.querySelector('.wf-step-card')).toBeNull();
  });

  it('classifies agent (token) vs deterministic steps via chip kind', () => {
    const { container } = renderDetail(mixedSteps);
    // 1 agent chip, 4 deterministic chips of distinct kinds (kind = type,
    // drives the icon).
    expect(container.querySelectorAll('.wf-pipe-chip[data-kind="agent"]').length).toBe(1);
    expect(container.querySelectorAll('.wf-pipe-chip[data-kind="api"]').length).toBe(1);
    expect(container.querySelectorAll('.wf-pipe-chip[data-kind="gate"]').length).toBe(1);
    expect(container.querySelectorAll('.wf-pipe-chip[data-kind="exec"]').length).toBe(1);
    expect(container.querySelectorAll('.wf-pipe-chip[data-kind="notify"]').length).toBe(1);
  });

  it('colors chips by the binary token class (agent vs deterministic), not per-type', () => {
    const { container } = renderDetail(mixedSteps);
    // Color is the headline signal: 1 agent (tokens) vs 4 deterministic
    // (0 token). The 4 deterministic steps share ONE color class despite
    // being 4 different types — that's the anti-rainbow fix.
    expect(container.querySelectorAll('.wf-pipe-chip[data-class="agent"]').length).toBe(1);
    expect(container.querySelectorAll('.wf-pipe-chip[data-class="determ"]').length).toBe(4);
  });

  it('shows the agent name only on Agent steps (not on deterministic ones)', () => {
    // Every fixture step carries agent: 'ClaudeCode' in its data, but only
    // the genuine Agent step should surface the agent identity — same
    // whitelist as the detail card, so both views read the same label.
    renderDetail(mixedSteps);
    const agentLine = screen.getByText('Claude Code');
    expect(agentLine).toHaveClass('wf-pipe-chip-agent');
    // Exactly one occurrence: the 4 deterministic steps must NOT render it.
    expect(screen.getAllByText('Claude Code').length).toBe(1);
  });

  it('expands to the full step cards when the toggle is clicked', () => {
    const { container } = renderDetail(mixedSteps);
    fireEvent.click(screen.getByTestId('wf-steps-toggle'));
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
    expect(container.querySelectorAll('.wf-step-card').length).toBe(5);
  });

  it('clicking a pipeline chip also expands the detail', () => {
    const { container } = renderDetail(mixedSteps);
    fireEvent.click(container.querySelector('.wf-pipe-chip') as HTMLElement);
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
  });
});

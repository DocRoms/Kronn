// Collapsed steps pipeline (WorkflowDetail).
//
// The Steps panel used to dump every per-step card (prompt + Test button)
// stacked vertically — heavy and rarely what you want at a glance,
// especially mid-run. It now collapses to a compact pipeline (number +
// kind icon + name) with an agent/deterministic count split, and "Voir en
// détails" reveals the legacy cards. These tests guard:
//   - collapsed by default (no detail cards rendered),
//   - the agent (token) vs deterministic classification drives chip colors,
//   - the toggle opens a focused single-step inspector.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

vi.mock('../../../lib/api', () => buildApiMock());

import { workflows as workflowsApi } from '../../../lib/api';
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

type DetailProps = React.ComponentProps<typeof WorkflowDetail>;

const renderDetail = (steps: WorkflowStep[], overrides: Partial<DetailProps> = {}) =>
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
      {...overrides}
    />
  );

describe('WorkflowDetail — collapsed steps pipeline', () => {
  it('renders collapsed by default — pipeline shown, full detail cards hidden', () => {
    const { container } = renderDetail(mixedSteps);
    expect(screen.getByTestId('wf-steps-section')).toBeInTheDocument();
    expect(screen.getByTestId('wf-branch-map')).toBeInTheDocument();
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

  it('shows the explicit step type on every compact index item', () => {
    const { container } = renderDetail(mixedSteps);
    const types = Array.from(container.querySelectorAll('.wf-pipe-chip-type'));
    expect(types).toHaveLength(mixedSteps.length);
    expect(types.map(type => type.textContent)).toEqual([
      'wiz.stepTypeAgent',
      'wiz.stepTypeApiCall',
      'wiz.stepTypeGate',
      'wiz.stepTypeExec',
      'wiz.stepTypeNotify',
    ]);
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

  it('opens one focused step card instead of dumping every detail card', () => {
    const { container } = renderDetail(mixedSteps);
    fireEvent.click(screen.getByTestId('wf-steps-toggle'));
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
    expect(container.querySelectorAll('.wf-step-card')).toHaveLength(1);
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('analyze');
  });

  it('clicking a pipeline chip opens that step in the inspector', () => {
    const { container } = renderDetail(mixedSteps);
    const chips = container.querySelectorAll('.wf-pipe-chip-open');
    fireEvent.click(chips[3] as HTMLElement);
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
    expect(container.querySelectorAll('.wf-step-card')).toHaveLength(1);
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('run_tests');
    expect(container.querySelector('.wf-pipe-chip[data-selected="true"]')).toHaveTextContent('run_tests');
  });

  it('navigates between focused steps with previous and next controls', () => {
    const { container } = renderDetail(mixedSteps);
    fireEvent.click(screen.getByTestId('wf-steps-toggle'));

    const previous = screen.getByRole('button', { name: 'wf.stepPrevious' });
    const next = screen.getByRole('button', { name: 'wf.stepNext' });
    expect(previous).toBeDisabled();
    fireEvent.click(next);
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('fetch_issue');
    expect(previous).not.toBeDisabled();
  });

  it('keeps the branch map visible and uses its nodes to open a focused step', () => {
    const branched = [
      mkStep({
        name: 'route',
        on_result: [{ contains: 'RETRY', action: { type: 'Goto', step_name: 'finish' } }],
      }),
      mkStep({ name: 'middle' }),
      mkStep({ name: 'finish' }),
    ];
    const { container } = renderDetail(branched);

    expect(screen.getByTestId('wf-branch-map')).toBeInTheDocument();
    expect(screen.queryByTestId('wf-steps-detail')).toBeNull();
    fireEvent.click(screen.getByTestId('wf-bm-node-2'));
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('finish');
  });

  it('shows a named Goto chip and opens its destination step', () => {
    const branched = [
      mkStep({
        name: 'route',
        on_result: [{ contains: 'RETRY', action: { type: 'Goto', step_name: 'finish' } }],
      }),
      mkStep({ name: 'middle' }),
      mkStep({ name: 'finish' }),
    ];
    const { container } = renderDetail(branched);
    fireEvent.click(screen.getByTestId('wf-steps-toggle'));

    const target = screen.getByRole('button', { name: 'wf.gotoTargetHint' });
    expect(target).toHaveTextContent('finish');
    fireEvent.click(target);
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('finish');
  });

  it('reveals every incoming Goto provenance and opens a source step', () => {
    const branched = [
      mkStep({
        name: 'route_a',
        on_result: [{ contains: 'SKIP', action: { type: 'Goto', step_name: 'finish' } }],
      }),
      mkStep({
        name: 'route_b',
        on_result: [{ contains: 'ABORT', action: { type: 'Goto', step_name: 'finish' } }],
      }),
      mkStep({ name: 'finish' }),
    ];
    const { container } = renderDetail(branched);
    fireEvent.click(screen.getByTestId('wf-bm-node-2'));

    const toggle = screen.getByRole('button', { name: /wf\.provenanceShow/ });
    fireEvent.click(toggle);
    const origins = screen.getByTestId('wf-step-origins');
    expect(origins).toHaveTextContent('route_a');
    expect(origins).toHaveTextContent('route_b');
    expect(origins.querySelectorAll('.wf-step-origin-chip')).toHaveLength(2);

    fireEvent.click(screen.getAllByRole('button', { name: 'wf.provenanceSourceHint' })[0]);
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('route_a');
  });

  it('uses the focused map, index and inspector design for a sub-workflow', async () => {
    const childWorkflow = {
      ...mkWorkflow([
        mkStep({ name: 'collect', step_type: { type: 'ApiCall' } }),
        mkStep({ name: 'review', step_type: { type: 'Agent' } }),
        mkStep({ name: 'publish', step_type: { type: 'Exec' } }),
      ]),
      id: 'child-wf',
      name: 'Reusable review',
    };
    vi.mocked(workflowsApi.get).mockResolvedValueOnce(childWorkflow);
    const parentStep = mkStep({
      name: 'delegate',
      step_type: { type: 'SubWorkflow' },
      sub_workflow_id: childWorkflow.id,
    });
    const { container } = renderDetail([parentStep]);

    fireEvent.click(screen.getByTestId('wf-steps-toggle'));
    const overview = await screen.findByTestId('wf-subworkflow-overview');

    expect(overview.querySelector('[data-testid="wf-branch-map"]')).toBeInTheDocument();
    expect(overview.querySelectorAll('.wf-pipe-chip')).toHaveLength(3);
    expect(overview.querySelectorAll('.wf-step-card')).toHaveLength(1);
    expect(overview.querySelector('.wf-step-card')).toHaveTextContent('collect');

    fireEvent.click(overview.querySelectorAll('.wf-pipe-chip-open')[1] as HTMLElement);
    expect(overview.querySelectorAll('.wf-step-card')).toHaveLength(1);
    expect(overview.querySelector('.wf-step-card')).toHaveTextContent('review');
    expect(container.querySelectorAll('.wf-step-card')).toHaveLength(2);
  });

  it('switches an Agent step directly from the collapsed pipeline', async () => {
    const onChangeStepAgent = vi.fn().mockResolvedValue(undefined);
    renderDetail(mixedSteps, {
      availableAgentTypes: ['ClaudeCode', 'Codex'],
      onChangeStepAgent,
    });

    const trigger = screen.getByLabelText('wf.stepAgentSwitchLabel');
    expect(trigger).toHaveClass('kr-agent-switch-btn');
    fireEvent.click(trigger);
    expect(trigger.closest('.kr-agent-switch')).toHaveAttribute('data-open', 'true');
    expect(screen.getByRole('menu').parentElement).toBe(document.body);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Codex' }));

    await waitFor(() => expect(onChangeStepAgent).toHaveBeenCalledWith(0, 'Codex'));
    expect(screen.queryByTestId('wf-steps-detail')).toBeNull();
  });
});

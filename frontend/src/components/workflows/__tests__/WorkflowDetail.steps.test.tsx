// Focused steps pipeline (WorkflowDetail).
//
// The Steps panel used to dump every per-step card (prompt + Test button)
// stacked vertically — heavy and rarely what you want at a glance,
// especially mid-run. It now shows a compact pipeline (number + kind icon +
// name) with an agent/deterministic count split and one focused inspector.
// These tests guard:
//   - the first selected step is open by default,
//   - the agent (token) vs deterministic classification drives chip colors,
//   - the toggle can still collapse the focused single-step inspector.

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
  mkStep({ id: '11111111-1111-4111-8111-111111111111', name: 'analyze', step_type: { type: 'Agent' } }),
  mkStep({ id: '22222222-2222-4222-8222-222222222222', name: 'fetch_issue', step_type: { type: 'ApiCall' } }),
  mkStep({ id: '33333333-3333-4333-8333-333333333333', name: 'plan_gate', step_type: { type: 'Gate' } }),
  mkStep({ id: '44444444-4444-4444-8444-444444444444', name: 'run_tests', step_type: { type: 'Exec' } }),
  mkStep({ id: '55555555-5555-4555-8555-555555555555', name: 'notify_done', step_type: { type: 'Notify' } }),
];

// Regression fixtures from workflow 90b8d76d-d226-4c51-acb0-bab4a08ef63e.
// These newer deterministic types used to fall through to the legacy Agent
// presentation in WorkflowDetail even though the runner handled them correctly.
const pagePipelineSteps = [
  mkStep({
    id: '9d8446d2-1216-484f-bb7d-8aac8f854426',
    name: 'collect-sources',
    step_type: { type: 'CollectApiData' },
    collect_api_data: {
      concurrent_limit: 5,
      sources: [
        { alias: 'adobe', quick_api_id: 'qa-adobe', quick_exec_id: '', required: true },
        { alias: 'releases', quick_api_id: 'qa-releases', quick_exec_id: '', required: true },
      ],
    },
  }),
  mkStep({
    id: '711bf7e5-9038-4cdc-8699-e7d8dc052f9f',
    name: 'shape',
    step_type: { type: 'TransformData' },
    transform_data: {
      input_from: 'steps.collect-sources.data',
      fields: [
        { target: 'rows', source: '$.sources.adobe.rows', operation: 'first' },
        { target: 'release', source: '$.sources.releases', operation: 'first' },
      ],
    },
  }),
  mkStep({
    id: 'a17746b5-a959-4a8f-b573-e41f93dcc5db',
    name: 'publish-page',
    step_type: { type: 'PublishPageData' },
    page_publish: {
      page_id: '8faf5138-8d45-4548-8f12-e0ac21e22cc5',
      writes: [{ dataset: 'daily', operation: 'replace', value_from: 'steps.shape.data' }],
    },
  }),
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

describe('WorkflowDetail — focused steps pipeline', () => {
  it('opens the selected first step by default without dumping every card', () => {
    const { container } = renderDetail(mixedSteps);
    expect(screen.getByTestId('wf-steps-section')).toBeInTheDocument();
    expect(screen.getByTestId('wf-branch-map')).toBeInTheDocument();
    // Compact pipeline shows one chip per step, with the step name.
    const chips = container.querySelectorAll('.wf-pipe-chip');
    expect(chips.length).toBe(5);
    expect(chips[0]).toHaveTextContent('analyze');
    expect(chips[4]).toHaveTextContent('notify_done');
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
    expect(container.querySelectorAll('.wf-step-card')).toHaveLength(1);
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('analyze');
  });

  it('still lets users collapse and reopen the focused inspector', () => {
    renderDetail(mixedSteps);
    const toggle = screen.getByTestId('wf-steps-toggle');

    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByTestId('wf-steps-detail')).toBeNull();

    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
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

  it('presents collect and transform steps as named deterministic operations', () => {
    const { container } = renderDetail(pagePipelineSteps);
    const types = Array.from(container.querySelectorAll('.wf-pipe-chip-type'));

    expect(types.map(type => type.textContent)).toEqual([
      'wiz.stepTypeCollectApiData',
      'wiz.stepTypeTransformData',
      'wiz.stepTypePublishPage',
    ]);
    expect(container.querySelectorAll('.wf-pipe-chip[data-class="determ"]')).toHaveLength(3);
    expect(container.querySelectorAll('.wf-pipe-chip[data-kind="agent"]')).toHaveLength(0);
    expect(container.querySelector('.wf-step-card')).toHaveAttribute('data-step-type', 'collect-data');
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('wiz.stepTypeCollectApiData');

    fireEvent.click(container.querySelectorAll('.wf-pipe-chip-open')[1] as HTMLElement);
    expect(container.querySelector('.wf-step-card')).toHaveAttribute('data-step-type', 'transform-data');
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('wiz.stepTypeTransformData');
  });

  it('opens the target Page from the Publish Page preview', () => {
    const onNavigatePage = vi.fn();
    const { container } = renderDetail(pagePipelineSteps, { onNavigatePage });
    fireEvent.click(container.querySelectorAll('.wf-pipe-chip-open')[2] as HTMLElement);

    expect(container.querySelector('.wf-step-card')).toHaveAttribute('data-step-type', 'page-data');
    fireEvent.click(screen.getByRole('button', { name: 'wiz.publishPageOpen' }));
    expect(onNavigatePage).toHaveBeenCalledWith('8faf5138-8d45-4548-8f12-e0ac21e22cc5');
  });

  it('shows a copyable canonical ID on every step', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    });
    const { container } = renderDetail(mixedSteps);
    const pipeline = container.querySelector('.wf-steps-pipeline');
    const ids = pipeline?.querySelectorAll<HTMLButtonElement>('.wf-step-id-pill') ?? [];
    expect(ids).toHaveLength(mixedSteps.length);
    expect(ids[1]).toHaveTextContent('#22222222');

    fireEvent.click(ids[1]);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith('22222222-2222-4222-8222-222222222222'));
    expect(ids[1]).toHaveAttribute('data-copied', 'true');
  });

  it('keeps the selected step ID visible in the expanded step card', () => {
    const { container } = renderDetail(mixedSteps);

    const card = container.querySelector('.wf-step-card');
    expect(card).toBeInTheDocument();
    expect(card?.querySelector('.wf-step-id-pill')).toHaveTextContent('#11111111');
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
    const { container } = renderDetail(mixedSteps);
    const agentLine = container.querySelector('.wf-pipe-chip-agent');
    expect(agentLine).toHaveClass('wf-pipe-chip-agent');
    expect(agentLine).toHaveTextContent('Claude Code');
    // Exactly one pipeline occurrence: the 4 deterministic steps must not
    // surface an agent, even though the open preview repeats this identity.
    expect(container.querySelectorAll('.wf-pipe-chip-agent')).toHaveLength(1);
  });

  it('opens one focused step card instead of dumping every detail card', () => {
    const { container } = renderDetail(mixedSteps);
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

  it('switches the selected step inspector from preview to the shared editor', () => {
    const { container } = renderDetail(mixedSteps);
    fireEvent.click(container.querySelectorAll('.wf-pipe-chip-open')[3] as HTMLElement);

    expect(screen.getByRole('tab', { name: 'markdown.preview' }))
      .toHaveAttribute('aria-selected', 'true');
    fireEvent.click(screen.getByRole('tab', { name: 'markdown.edit' }));

    expect(screen.getByRole('tab', { name: 'markdown.edit' }))
      .toHaveAttribute('aria-selected', 'true');
    const focusedEditor = container.querySelector('.wf-wizard-card[data-focused-step="true"]');
    expect(focusedEditor).toBeInTheDocument();
    expect(focusedEditor?.querySelector('[data-wizard-step-index="3"] input'))
      .toHaveValue('run_tests');
    expect(focusedEditor?.querySelector('[data-wizard-step-index="0"]')).toBeNull();
  });

  it('saves a focused step through the regular workflow update and refreshes the preview', async () => {
    const onRefresh = vi.fn();
    vi.mocked(workflowsApi.update).mockClear();
    const { container } = renderDetail([mixedSteps[0]], { onRefresh });
    fireEvent.click(screen.getByRole('tab', { name: 'markdown.edit' }));

    const nameInput = container.querySelector<HTMLInputElement>(
      '[data-wizard-step-index="0"] input[aria-label="wiz.stepName"]',
    );
    expect(nameInput).toBeInTheDocument();
    fireEvent.change(nameInput!, { target: { value: 'analyze_issue' } });
    fireEvent.click(screen.getByText('wiz.save'));

    await waitFor(() => expect(workflowsApi.update).toHaveBeenCalledWith(
      'wf-1',
      expect.objectContaining({
        steps: expect.arrayContaining([
          expect.objectContaining({
            id: '11111111-1111-4111-8111-111111111111',
            name: 'analyze_issue',
          }),
        ]),
      }),
    ));
    await waitFor(() => expect(onRefresh).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('tab', { name: 'markdown.preview' }))
      .toHaveAttribute('aria-selected', 'true');
  });

  it('cancels focused editing without persisting the draft', () => {
    vi.mocked(workflowsApi.update).mockClear();
    const { container } = renderDetail(mixedSteps);
    fireEvent.click(screen.getByRole('tab', { name: 'markdown.edit' }));

    const nameInput = container.querySelector<HTMLInputElement>(
      '[data-wizard-step-index="0"] input[aria-label="wiz.stepName"]',
    );
    fireEvent.change(nameInput!, { target: { value: 'discard_me' } });
    fireEvent.click(screen.getByText('common.cancel'));

    expect(workflowsApi.update).not.toHaveBeenCalled();
    expect(screen.getByRole('tab', { name: 'markdown.preview' }))
      .toHaveAttribute('aria-selected', 'true');
    expect(container.querySelector('.wf-step-card')).toHaveTextContent('analyze');
  });

  it('navigates between focused steps with previous and next controls', () => {
    const { container } = renderDetail(mixedSteps);

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
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
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

  it('switches an Agent step directly from the compact pipeline', async () => {
    const onChangeStepAgent = vi.fn().mockResolvedValue(undefined);
    const { container } = renderDetail(mixedSteps, {
      availableAgentTypes: ['ClaudeCode', 'Codex'],
      onChangeStepAgent,
    });

    const trigger = container.querySelector<HTMLButtonElement>(
      '.wf-steps-pipeline [aria-label="wf.stepAgentSwitchLabel"]',
    );
    expect(trigger).not.toBeNull();
    expect(trigger).toHaveClass('kr-agent-switch-btn');
    fireEvent.click(trigger!);
    expect(trigger!.closest('.kr-agent-switch')).toHaveAttribute('data-open', 'true');
    expect(screen.getByRole('menu').parentElement).toBe(document.body);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Codex · disc.tier.default' }));

    await waitFor(() => expect(onChangeStepAgent).toHaveBeenCalledWith(0, 'Codex', 'default'));
    expect(screen.getByTestId('wf-steps-detail')).toBeInTheDocument();
  });
});

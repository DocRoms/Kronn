// KT-1696 — the shadows and the merged result in the workflow overview: sub-rows
// under the Agent step with their toggles, the lines on the branch map, the
// header button, and the runs list.
// KT-1707 — a doubled step opens with a highlighted Principal row, which hosts the
// step's own agent picker; a step without shadows keeps its single agent line.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

const { stepShadows } = vi.hoisted(() => ({ stepShadows: vi.fn() }));
vi.mock('../../../lib/api', () => buildApiMock({ workflows: { stepShadows: stepShadows as never } }));

import { WorkflowDetail } from '../WorkflowDetail';
import type { ShadowEdit } from '../../../lib/shadowMerge';
import type { StepShadowComparison, Workflow, WorkflowRun, WorkflowStep } from '../../../types/generated';

const mkStep = (partial: Partial<WorkflowStep> & { name: string }): WorkflowStep => ({
  step_type: { type: 'Agent' }, description: null, agent: 'Ollama', prompt_template: 'do', mode: { type: 'Normal' },
  output_format: { type: 'FreeText' }, mcp_config_ids: [], agent_settings: { model: 'qwen3.8:27b-mlx' }, on_result: [],
  stall_timeout_secs: null, retry: null, skill_ids: [], directive_ids: [], profile_ids: [], delay_after_secs: null,
  ...partial,
} as unknown as WorkflowStep);

const argon = { id: 'argon', label: 'Argon', enabled: true, agent: { agent: 'LiteLlm' as const, model: 'gemini-4-argon' } };
const twin = { id: 'twin', label: 'Qwen identique', enabled: true };

const steps = [
  mkStep({ name: 'collect', agent: 'ClaudeCode', agent_settings: null }),
  mkStep({ name: 'advise', shadows: [argon, twin], shadow_merge: { enabled: true, shadows: ['argon'], timeout_secs: 600 } }),
  mkStep({ name: 'publish', agent: 'ClaudeCode', agent_settings: null }),
];

const mkWorkflow = (overrides: Partial<Workflow> = {}): Workflow => ({
  id: 'wf-1', name: 'Top 5xx', project_id: null, trigger: { type: 'Manual' }, steps, actions: [],
  safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false }, workspace_config: null,
  concurrency_limit: null, enabled: true, created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
  ...overrides,
} as unknown as Workflow);

const mkRun = (id: string): WorkflowRun => ({
  id, workflow_id: 'wf-1', status: 'Success', trigger_context: null, step_results: [], tokens_used: 0, workspace_path: null,
  started_at: new Date('2026-05-29T10:00:00Z').toISOString(), finished_at: new Date('2026-05-29T10:01:00Z').toISOString(),
  produced_branches: [],
} as unknown as WorkflowRun);

type Props = React.ComponentProps<typeof WorkflowDetail>;

function renderDetail(overrides: Partial<Props> = {}) {
  return render(
    <WorkflowDetail
      workflow={mkWorkflow()} runs={[]} liveRun={null} onTrigger={vi.fn()} onRefresh={vi.fn()} onEdit={vi.fn()}
      onDeleteRun={vi.fn()} onDeleteAllRuns={vi.fn()} triggering={false} {...overrides}
    />,
  );
}

afterEach(() => { cleanup(); stepShadows.mockReset(); });

describe('WorkflowDetail — shadows in the step list', () => {
  it('puts one sub-row per shadow under the Agent step that has them, and none under the others', () => {
    const { container } = renderDetail();
    const chips = container.querySelectorAll('.wf-pipe-chip');
    expect(chips).toHaveLength(3);
    expect(within(chips[1] as HTMLElement).getByTestId('wf-shadow-rows-1-argon')).toBeInTheDocument();
    expect(within(chips[1] as HTMLElement).getByTestId('wf-shadow-rows-1-twin')).toBeInTheDocument();
    expect(within(chips[0] as HTMLElement).queryByTestId(/wf-shadow-rows/)).toBeNull();
    expect(within(chips[2] as HTMLElement).queryByTestId(/wf-shadow-rows/)).toBeNull();
    // Without a way to save, the toggles only show.
    expect(screen.getByTestId('wf-shadow-rows-1-twin-merge')).toBeDisabled();
  });

  it('summarises the merged result towards the next step', () => {
    renderDetail();
    expect(screen.getByTestId('wf-shadow-rows-1-merged')).toHaveTextContent('wf.shadow.rows.summary');
  });

  it('hands an edit to the page, which saves it at once', () => {
    const onChangeStepShadows = vi.fn<(index: number, edit: ShadowEdit) => Promise<void>>().mockResolvedValue(undefined);
    renderDetail({ onChangeStepShadows });
    fireEvent.click(screen.getByTestId('wf-shadow-rows-1-twin-merge'));
    expect(onChangeStepShadows).toHaveBeenCalledTimes(1);
    const [index, edit] = onChangeStepShadows.mock.calls[0];
    expect(index).toBe(1);
    expect(edit.merge).toEqual({ enabled: true, shadows: ['argon', 'twin'], timeout_secs: 600 });
  });

  it('the branch map no longer says « no branches » when a step is doubled', () => {
    renderDetail();
    expect(screen.getByTestId('wf-bm-shadow-tag-1')).toHaveTextContent('⑂ 2');
    expect(screen.getByTestId('wf-bm-merge-tag-1')).toHaveTextContent('⊕');
    expect(screen.getByTestId('wf-bm-legend-linear')).toHaveTextContent('wf.branchMap.linearDoubledOne');
    expect(screen.getByTestId('wf-bm-legend-linear')).not.toHaveTextContent(/wf\.branchMap\.linear(?!Doubled)/);
  });

  it('the header button counts shadows, steps and the merge', () => {
    renderDetail();
    expect(screen.getByTestId('wf-shadow-entry')).toHaveTextContent('wf.shadow.entryShadowsMany · wf.shadow.entryStepsOne · wf.shadow.entryMerge');
  });
});

describe('WorkflowDetail — the Principal row (KT-1707)', () => {
  const chipsOf = (container: HTMLElement) => Array.from(container.querySelectorAll<HTMLElement>('.wf-pipe-chip'));

  it('opens the block with the Principal row, hosting the step picker, before the shadow rows and the merged line', () => {
    const { container } = renderDetail();
    const chip = chipsOf(container)[1];
    const principal = within(chip).getByTestId('wf-shadow-rows-1-principal');
    expect(within(principal).getByTestId('wf-shadow-rows-1-principal-agent')).toHaveTextContent('Ollama');
    expect(within(principal).getByTestId('wf-shadow-rows-1-principal-agent')).toHaveTextContent('qwen3.8:27b-mlx');
    expect(within(principal).getByText('wf.shadow.rows.transport.local')).toBeInTheDocument();
    expect(within(principal).getByTestId('wf-shadow-rows-1-principal-active')).toHaveTextContent('wf.shadow.rows.alwaysActive');
    expect(within(principal).queryByRole('switch')).toBeNull();
    const follows = (a: HTMLElement, b: HTMLElement) => !!(a.compareDocumentPosition(b) & Node.DOCUMENT_POSITION_FOLLOWING);
    expect(follows(principal, screen.getByTestId('wf-shadow-rows-1-argon'))).toBe(true);
    expect(follows(screen.getByTestId('wf-shadow-rows-1-twin'), screen.getByTestId('wf-shadow-rows-1-merged'))).toBe(true);
    // The picker moved: it is no longer on the card's single agent line.
    expect(chip.querySelector('.wf-pipe-chip-meta .wf-pipe-chip-agent')).toBeNull();
  });

  it('keeps the single agent line, with no block, on a step without shadows', () => {
    const { container } = renderDetail();
    const [collect, , publish] = chipsOf(container);
    for (const chip of [collect, publish]) {
      expect(chip.querySelector('.wf-pipe-chip-meta .wf-pipe-chip-agent')).not.toBeNull();
      expect(within(chip).queryByTestId(/principal/)).toBeNull();
    }
  });

  it('turns the line into the block with the first shadow, and back with the last one', () => {
    const plain = [steps[0], mkStep({ name: 'advise' }), steps[2]];
    const withOne = [steps[0], mkStep({ name: 'advise', shadows: [twin] }), steps[2]];
    const { container, rerender } = renderDetail({ workflow: mkWorkflow({ steps: plain }) });
    const advise = () => chipsOf(container)[1];
    expect(advise().querySelector('.wf-pipe-chip-meta .wf-pipe-chip-agent')).not.toBeNull();
    expect(within(advise()).queryByTestId('wf-shadow-rows-1-principal')).toBeNull();
    rerender(<WorkflowDetail workflow={mkWorkflow({ steps: withOne })} runs={[]} liveRun={null} onTrigger={vi.fn()} onRefresh={vi.fn()} onEdit={vi.fn()} onDeleteRun={vi.fn()} onDeleteAllRuns={vi.fn()} triggering={false} />);
    expect(within(advise()).getByTestId('wf-shadow-rows-1-principal')).toBeInTheDocument();
    expect(advise().querySelector('.wf-pipe-chip-meta .wf-pipe-chip-agent')).toBeNull();
    rerender(<WorkflowDetail workflow={mkWorkflow({ steps: plain })} runs={[]} liveRun={null} onTrigger={vi.fn()} onRefresh={vi.fn()} onEdit={vi.fn()} onDeleteRun={vi.fn()} onDeleteAllRuns={vi.fn()} triggering={false} />);
    expect(within(advise()).queryByTestId('wf-shadow-rows-1-principal')).toBeNull();
    expect(advise().querySelector('.wf-pipe-chip-meta .wf-pipe-chip-agent')).not.toBeNull();
  });

  it('lists the principal line on the branch map before the shadow lines', () => {
    renderDetail();
    const principal = screen.getByTestId('wf-bm-principal-1');
    expect(principal).toHaveTextContent('wf.branchMap.principal');
    expect(Number(principal.getAttribute('y'))).toBeLessThan(Number(screen.getByTestId('wf-bm-shadow-1-argon').getAttribute('y')));
  });

  it('changes the main agent from the Principal row through the existing save path, and keeps the row on a failed save', async () => {
    const onChangeStepAgent = vi.fn().mockRejectedValue(new Error('save failed'));
    const { container } = renderDetail({ onChangeStepAgent, availableAgentTypes: ['Ollama', 'ClaudeCode'] });
    const principal = within(chipsOf(container)[1]).getByTestId('wf-shadow-rows-1-principal');
    const trigger = within(principal).getByRole('button', { name: /wf\.stepAgentSwitchLabel/ });
    expect(trigger).toHaveTextContent('qwen3.8:27b-mlx');
    fireEvent.click(trigger);
    fireEvent.click(screen.getByRole('menuitem', { name: 'Claude Code · disc.tier.default' }));
    await waitFor(() => expect(onChangeStepAgent).toHaveBeenCalledWith(1, 'ClaudeCode', 'default', null));
    // The page owns the saved workflow: a rejected save leaves the row as it was, picker still open.
    expect(within(chipsOf(container)[1]).getByTestId('wf-shadow-rows-1-principal-agent')).toHaveTextContent('Ollama');
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
  });
});

describe('WorkflowDetail — runs list', () => {
  const report = (status: string, missing: number) => ({ status, selected: ['argon'], contributors: [], missing: Array.from({ length: missing }, () => ({})) });
  const comparison = (merge?: unknown): StepShadowComparison => ({
    step_name: 'advise', result_index: 1, main: {}, shadows: [{}, {}], ...(merge ? { merge } : {}),
  }) as unknown as StepShadowComparison;

  it('tags a run with ⑂ N · ⊕ ok when its merge went whole', async () => {
    stepShadows.mockResolvedValue([comparison(report('merged', 0))]);
    renderDetail({ runs: [mkRun('run-1')] });
    const tag = await screen.findByTestId('wf-run-shadows');
    await waitFor(() => expect(tag).toHaveTextContent('⑂ 2 · ⊕ wf.shadow.rows.runMergeOk'));
    expect(tag).toHaveAttribute('data-merge', 'ok');
  });

  it('says partial when a shadow did not make it into the merge', async () => {
    stepShadows.mockResolvedValue([comparison(report('merged', 1))]);
    renderDetail({ runs: [mkRun('run-1')] });
    const tag = await screen.findByTestId('wf-run-shadows');
    expect(tag).toHaveTextContent('⊕ wf.shadow.rows.runMergePartial');
    expect(tag).toHaveAttribute('data-merge', 'partial');
  });

  it('shows only ⑂ N for a run whose shadows were compared, not merged', async () => {
    stepShadows.mockResolvedValue([comparison()]);
    renderDetail({ runs: [mkRun('run-1')] });
    const tag = await screen.findByTestId('wf-run-shadows');
    expect(tag.textContent).toBe('⑂ 2');
  });

  it('leaves a run without shadows untagged', async () => {
    stepShadows.mockResolvedValue([]);
    renderDetail({ runs: [mkRun('run-1')] });
    await waitFor(() => expect(stepShadows).toHaveBeenCalled());
    expect(screen.queryByTestId('wf-run-shadows')).toBeNull();
  });

  it('does not ask for shadows of a workflow that has none', () => {
    renderDetail({ workflow: mkWorkflow({ steps: [steps[0], steps[2]] }), runs: [mkRun('run-1')] });
    expect(screen.queryByTestId('wf-run-shadows')).toBeNull();
    expect(stepShadows).not.toHaveBeenCalled();
  });
});

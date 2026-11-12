// KT-1697 — the Decide step card of the workflow overview picks its decision
// model like the Agent step card picks its model.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';

const catalog = vi.hoisted(() => ({
  targets: [{
    runtime_target_id: 'agent:ollama', agent_type: 'Ollama', stale: false, live_refresh_ok: true,
    models: ['clef-flash:9b', 'clef-pro:27b'].map(model_id => ({
      model_id, display_name: model_id, display_alias: null, capabilities: ['decision'],
      runtime_target_id: 'agent:ollama', availability: 'available',
    })),
  }],
}));

vi.mock('../../../lib/api', () => buildApiMock({
  modelCatalogApi: { list: vi.fn().mockResolvedValue(catalog) },
}));

import { modelCatalogApi } from '../../../lib/api';
import { WorkflowDetail } from '../WorkflowDetail';
import type { AgentsConfig, Workflow, WorkflowStep } from '../../../types/generated';

const decideStep = (model?: string): WorkflowStep => ({
  id: '11111111-1111-4111-8111-111111111111',
  name: 'triage',
  step_type: { type: 'Decide' },
  agent: 'ClaudeCode',
  prompt_template: '',
  mode: { type: 'Normal' },
  output_format: { type: 'FreeText' },
  mcp_config_ids: [],
  on_result: [],
  skill_ids: [],
  directive_ids: [],
  profile_ids: [],
  decide: { state: '{{previous_step.output}}', questions: [{ id: 'ship', kind: 'noul', instructions: 'Ready?' }], ...(model ? { model } : {}) },
} as unknown as WorkflowStep);

const workflow = (step: WorkflowStep): Workflow => ({
  id: 'wf-1', name: 'Triage', project_id: null, trigger: { type: 'Manual' }, steps: [step], actions: [],
  safety: { sandbox: false, max_files: null, max_lines: null, require_approval: false },
  workspace_config: null, concurrency_limit: null, enabled: true,
  created_at: new Date().toISOString(), updated_at: new Date().toISOString(),
} as unknown as Workflow);

const agentAccess = { model_tiers: { ollama: { decision: 'clef-flash:9b' } } } as unknown as AgentsConfig;

const renderDetail = (step: WorkflowStep, onChangeStepDecisionModel?: (i: number, c: unknown) => Promise<void>) =>
  render(
    <WorkflowDetail
      workflow={workflow(step)}
      runs={[]}
      liveRun={null}
      onTrigger={() => {}}
      onRefresh={() => {}}
      onEdit={() => {}}
      onDeleteRun={() => {}}
      onDeleteAllRuns={() => {}}
      triggering={false}
      agentAccess={agentAccess}
      onChangeStepDecisionModel={onChangeStepDecisionModel as never}
    />
  );

describe('WorkflowDetail — Decide step model picker (KT-1697)', () => {
  it('shows the configured model with its transport badge on the pipeline chip and on the card', async () => {
    renderDetail(decideStep(), vi.fn());
    await waitFor(() => expect(screen.getAllByTestId('decision-model-picker')).toHaveLength(2));
    for (const name of screen.getAllByTestId('decision-model-name')) expect(name).toHaveTextContent('clef-flash:9b');
    for (const badge of screen.getAllByTestId('decision-transport')) {
      expect(badge).toHaveTextContent('decisionPicker.transport.local');
    }
    expect(modelCatalogApi.list).toHaveBeenCalled();
  });

  it('shows the step\'s own model, lists the default first and saves the pick at once', async () => {
    const onChange = vi.fn().mockResolvedValue(undefined);
    renderDetail(decideStep('clef-pro:27b'), onChange);
    const [chip] = await screen.findAllByTestId('decision-model-picker');
    expect(chip).toHaveTextContent('clef-pro:27b');
    fireEvent.click(chip);
    const items = await screen.findAllByRole('menuitem');
    expect(items[0]).toHaveAttribute('data-testid', 'decision-option-default');
    fireEvent.click(screen.getByTestId('decision-option-default'));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith(0, { model: null, connectionId: null }));
  });

  it('is read-only without a save handler', async () => {
    renderDetail(decideStep());
    await waitFor(() => expect(screen.getAllByTestId('decision-model-static').length).toBeGreaterThan(0));
    expect(screen.queryByTestId('decision-model-picker')).toBeNull();
  });
});

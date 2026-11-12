// KT-1696 — in the editor, the shadows of an Agent step sit right under the
// step's agent, outside the « Advanced » section, and edit the same fields.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { buildApiMock } from '../../../test/apiMock';
import type { Workflow, WorkflowStep } from '../../../types/generated';

const { createMock, updateMock } = vi.hoisted(() => ({ createMock: vi.fn(), updateMock: vi.fn() }));

vi.mock('../../../lib/api', () => buildApiMock({ workflows: { create: createMock as never, update: updateMock as never } }));

// Key-passthrough i18n so assertions are locale-stable.
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) => (args.length > 0 ? `${key}:${args.join(',')}` : key),
    locale: 'fr',
    setLocale: () => {},
  }),
}));

import { WorkflowWizard } from '../WorkflowWizard';

const mkStep = (over: Partial<WorkflowStep> = {}): WorkflowStep => ({
  id: '11111111-1111-4111-8111-111111111111', name: 'advise', step_type: { type: 'Agent' }, description: null,
  agent: 'ClaudeCode', prompt_template: 'do the thing', mode: { type: 'Normal' }, output_format: { type: 'Structured' }, ...over,
});

const mkWorkflow = (steps: WorkflowStep[]): Workflow => ({
  id: 'wf-1', name: 'Top 5xx', project_id: null, trigger: { type: 'Manual' }, steps, actions: [], enabled: true,
  created_at: '2026-05-01T00:00:00Z', updated_at: '2026-05-01T00:00:00Z',
} as unknown as Workflow);

const twin = { id: 'twin', label: 'Qwen identique', enabled: true };
const argon = { id: 'argon', label: 'Argon', enabled: true, agent: { agent: 'Ollama' as const, model: 'qwen' } };

const renderWizard = (steps: WorkflowStep[]) => {
  const props: ComponentProps<typeof WorkflowWizard> = {
    projects: [], onDone: vi.fn(), onCancel: vi.fn(), installedAgentTypes: ['ClaudeCode', 'Ollama'], editWorkflow: mkWorkflow(steps),
  };
  return render(<WorkflowWizard {...props} />);
};

const toStepsPage = (steps: WorkflowStep[]) => {
  renderWizard(steps);
  fireEvent.click(screen.getByText('wiz.next')); // Infos → Trigger
  fireEvent.click(screen.getByText('wiz.next')); // Trigger → Steps
};

beforeEach(() => { createMock.mockReset().mockResolvedValue({}); updateMock.mockReset().mockResolvedValue({}); });
afterEach(cleanup);

describe('WorkflowWizard — shadow sub-rows under the step agent (KT-1696)', () => {
  const steps = () => [
    mkStep({ shadows: [argon, twin], shadow_merge: { enabled: true, shadows: ['argon'] } }),
    mkStep({ id: '22222222-2222-4222-8222-222222222222', name: 'publish' }),
  ];

  it('shows the rows directly under the agent picker, with « Advanced » closed', () => {
    toStepsPage(steps());
    expect(screen.queryByTestId('wf-step-shadows-0')).toBeNull();
    const rows = screen.getByTestId('wf-wiz-shadow-rows-0');
    expect(rows.closest('.wf-advanced-panel')).toBeNull();
    // Right under the agent routing of its step, in the same block.
    const routing = rows.closest('.wf-step-agent-routing');
    expect(routing).not.toBeNull();
    expect(routing?.querySelector('.kr-agent-switch-btn, .kr-agent-switch-static')).not.toBeNull();
    expect(screen.getByTestId('wf-wiz-shadow-rows-0-argon')).toBeInTheDocument();
    expect(screen.getByTestId('wf-wiz-shadow-rows-0-twin')).toBeInTheDocument();
    expect(screen.getByTestId('wf-wiz-shadow-rows-0-merged')).toBeInTheDocument();
  });

  it('offers + Shadow on a step that has none, and nothing on a step that is not an Agent step', () => {
    toStepsPage([mkStep(), mkStep({ name: 'run', step_type: { type: 'Exec' } })]);
    expect(screen.getByTestId('wf-wiz-shadow-rows-0-add')).toBeInTheDocument();
    expect(screen.queryByTestId('wf-wiz-shadow-rows-1-add')).toBeNull();
  });

  it('a toggle edits the step, and the workflow is saved with an explicit merge list', async () => {
    toStepsPage(steps());
    fireEvent.click(screen.getByTestId('wf-wiz-shadow-rows-0-twin-merge'));
    expect(screen.getByTestId('wf-wiz-shadow-rows-0-twin-merge')).toHaveAttribute('aria-checked', 'true');
    // Walk to the end and save.
    for (let i = 0; i < 6 && !screen.queryByText('wiz.save'); i += 1) {
      const next = screen.queryByText('wiz.next');
      if (!next) break;
      fireEvent.click(next);
    }
    fireEvent.click(await screen.findByText('wiz.save'));
    await vi.waitFor(() => expect(updateMock).toHaveBeenCalled());
    const body = updateMock.mock.calls[0][1];
    expect(body.steps[0].shadow_merge).toEqual({ enabled: true, shadows: ['argon', 'twin'] });
  });

  it('keeps the detailed shadow settings in « Advanced »', () => {
    toStepsPage(steps());
    fireEvent.click(screen.getAllByText(/^wiz\.advanced/)[0]);
    expect(screen.getByTestId('wf-step-shadows-0')).toBeInTheDocument();
  });

  it('simple mode shows the rows too', () => {
    renderWizard([mkStep({ shadows: [twin] })]);
    fireEvent.click(screen.getByText('wiz.next')); // Infos → Trigger
    expect(screen.getByTestId('wf-wiz-shadow-rows-simple-twin')).toBeInTheDocument();
  });
});

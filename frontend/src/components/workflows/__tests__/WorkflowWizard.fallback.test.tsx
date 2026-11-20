// KT-1693 — the step's fallback override sits under the step's agent/mode choice,
// visible without opening the « advanced » section (simple mode and the steps
// list alike), and is the same component as the agent cards' rows.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, within } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { buildApiMock } from '../../../test/apiMock';
import type { Project, Workflow, WorkflowStep } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length > 0 ? `${key}:${args.join(',')}` : key,
    locale: 'fr',
    setLocale: () => {},
  }),
}));

import { WorkflowWizard } from '../WorkflowWizard';

const project: Project = {
  id: 'proj-1',
  name: 'ProjectAlpha',
  path: '/tmp/alpha',
  repo_url: null,
  token_override: null,
  ai_config: { detected: false, configs: [] },
  audit_status: 'None' as Project['audit_status'],
  ai_todo_count: 0, tech_debt_count: 0, needs_docs_migration: false, path_exists: true,
  created_at: '2026-05-01T00:00:00Z',
  updated_at: '2026-05-01T00:00:00Z',
};

const step = (over: Partial<WorkflowStep> = {}): WorkflowStep => ({
  id: '11111111-1111-4111-8111-111111111111',
  name: 'main',
  step_type: { type: 'Agent' },
  description: null,
  agent: 'ClaudeCode',
  prompt_template: 'do the thing',
  mode: { type: 'Normal' },
  output_format: { type: 'Structured' },
  ...over,
});

const workflow = (steps: WorkflowStep[]): Workflow => ({
  id: 'wf-1',
  name: 'ExistingWorkflow',
  project_id: 'proj-1',
  trigger: { type: 'Cron', schedule: '*/5 * * * *' },
  steps,
  actions: [],
  enabled: true,
  created_at: '2026-05-01T00:00:00Z',
  updated_at: '2026-05-01T00:00:00Z',
} as unknown as Workflow);

const props: ComponentProps<typeof WorkflowWizard> = {
  projects: [project],
  onDone: vi.fn(),
  onCancel: vi.fn(),
  installedAgentTypes: ['ClaudeCode', 'Codex'],
};

beforeEach(() => {
  vi.stubGlobal('confirm', vi.fn(() => true));
});
afterEach(cleanup);

describe('the fallback override of a workflow step', () => {
  it('shows under the agent and mode choice of a step, with « advanced » closed', () => {
    render(<WorkflowWizard {...props} editWorkflow={workflow([step()])} />);
    fireEvent.click(screen.getByText('wiz.next')); // Infos → Trigger
    fireEvent.click(screen.getByText('wiz.next')); // Trigger → Steps

    // « Advanced » is still collapsed, and the override is there.
    expect(document.querySelector('.wf-advanced-panel')).toBeNull();
    const override = screen.getByTestId('wiz-step-0-fallback');
    expect(override).toBeTruthy();
    expect(within(override).getByTestId('wiz-step-0-fallback-row-add')).toBeTruthy();

    // It lives in the agent routing block, after the agent/mode picker it depends on.
    const routing = document.querySelector('.wf-step-agent-routing') as HTMLElement;
    expect(routing.contains(override)).toBe(true);
    const picker = routing.querySelector('.kr-agent-switch') as HTMLElement;
    expect(picker.compareDocumentPosition(override) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('is not repeated once « advanced » is open', () => {
    render(<WorkflowWizard {...props} editWorkflow={workflow([step()])} />);
    fireEvent.click(screen.getByText('wiz.next'));
    fireEvent.click(screen.getByText('wiz.next'));
    fireEvent.click(screen.getByRole('button', { name: /^wiz\.advanced/ }));

    const panel = document.querySelector('.wf-advanced-panel') as HTMLElement;
    expect(panel).toBeTruthy();
    expect(within(panel).queryByTestId('wiz-step-0-fallback')).toBeNull();
    expect(screen.getAllByTestId('wiz-step-0-fallback')).toHaveLength(1);
  });

  it('shows on the simple mode step too, right after the agent picker', () => {
    render(<WorkflowWizard {...props} />);
    fireEvent.change(screen.getByLabelText('wiz.name'), { target: { value: 'Weekly report' } });
    fireEvent.click(screen.getByText('wiz.next'));

    const override = screen.getByTestId('wiz-step-0-fallback');
    const picker = document.querySelector('.wf-wizard-agent-tier') as HTMLElement;
    expect(picker.contains(override)).toBe(false);
    expect(picker.compareDocumentPosition(override) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  it('is only offered on Agent steps', () => {
    render(
      <WorkflowWizard
        {...props}
        editWorkflow={workflow([step({ step_type: { type: 'Notify' } as WorkflowStep['step_type'] })])}
      />,
    );
    fireEvent.click(screen.getByText('wiz.next'));
    fireEvent.click(screen.getByText('wiz.next'));
    expect(screen.queryByTestId('wiz-step-0-fallback')).toBeNull();
  });
});

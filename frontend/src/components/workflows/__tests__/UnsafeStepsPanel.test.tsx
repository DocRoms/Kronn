import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import type { UnsafeExecStep, Workflow, WorkflowBlocker, WorkflowReadiness } from '../../../types/generated';

const unsafeSteps = vi.fn();
const readiness = vi.fn();
vi.mock('../../../lib/api', () => ({
  workflows: {
    unsafeSteps: (...args: unknown[]) => unsafeSteps(...args),
    readiness: (...args: unknown[]) => readiness(...args),
  },
}));
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: unknown[]) => [key, ...args].join('|') }),
}));

import { UnsafeStepsPanel } from '../UnsafeStepsPanel';

const workflow = { id: 'wf-1', name: 'PR review' } as Workflow;
const fixable: UnsafeExecStep = {
  step_name: 'greet',
  on_failure: false,
  phase: 'main',
  command: 'bash',
  args: ['-c', 'echo {{issue.title}}'],
  placeholder: '{{issue.title}}',
  reason: 'inline_code_interpolation',
  suggested_args: ['-c', 'echo "$1"', '_', '{{issue.title}}'],
  manual_fix: null,
  agent_written: false,
};
const manual: UnsafeExecStep = {
  ...fixable,
  step_name: 'heredoc',
  args: ['-c', 'cat <<EOF\n{{issue.title}}\nEOF'],
  suggested_args: null,
  manual_fix: 'correction manuelle requise : le script contient un heredoc',
};

describe('UnsafeStepsPanel', () => {
  beforeEach(() => {
    unsafeSteps.mockReset();
    readiness.mockReset();
    readiness.mockResolvedValue(null);
  });

  it('renders nothing when every step is safe', async () => {
    unsafeSteps.mockResolvedValue([]);
    const { container } = render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    await waitFor(() => expect(unsafeSteps).toHaveBeenCalledWith('wf-1'));
    expect(container).toBeEmptyDOMElement();
  });

  it('shows the diff on request and applies only on click', async () => {
    unsafeSteps.mockResolvedValue([fixable]);
    const onApply = vi.fn().mockResolvedValue(undefined);
    render(<UnsafeStepsPanel workflow={workflow} onApply={onApply} />);

    await screen.findByText('{{issue.title}}');
    expect(screen.queryByText('wf.unsafeApply')).toBeNull();
    fireEvent.click(screen.getByText('wf.unsafeSuggest'));
    expect(screen.getByText(/\+ bash \["-c","echo \\"\$1\\"","_","\{\{issue.title\}\}"\]/)).toBeDefined();
    expect(onApply).not.toHaveBeenCalled();

    fireEvent.click(screen.getByText('wf.unsafeApply'));
    await waitFor(() => expect(onApply).toHaveBeenCalledWith(fixable));
  });

  it('shows a line an agent wrote with its approval, and approves on click', async () => {
    const pending: UnsafeExecStep = {
      ...fixable,
      step_name: 'plan',
      args: ['-c', 'terraform plan "$1"', '_', '{{x}}'],
      placeholder: '{{x}}',
      reason: 'unmodelled_program',
      suggested_args: null,
      manual_fix: 'écrite par un agent',
      agent_written: true,
    };
    unsafeSteps.mockResolvedValue([pending]);
    const onApprove = vi.fn().mockResolvedValue(undefined);
    render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} onApprove={onApprove} />);

    expect(await screen.findByText(/wf\.unsafeAgentWritten/)).toBeDefined();
    // A missing approval is not presented as a value inside code.
    expect(screen.getByRole('heading').textContent).toContain('wf.approvalTitle');
    expect(screen.getByText('wf.approvalIntro')).toBeDefined();
    expect(screen.queryByText('wf.unsafeIntro')).toBeNull();
    expect(screen.queryByText('wf.unsafeHow')).toBeNull();
    expect(screen.queryByLabelText('wf.unsafeAgentPromptLabel')).toBeNull();
    fireEvent.click(screen.getByText('wf.approvalReview'));
    expect(screen.getByText('bash ["-c","terraform plan \\"$1\\"","_","{{x}}"]')).toBeDefined();
    fireEvent.click(screen.getByText('wf.unsafeApprove'));
    await waitFor(() => expect(onApprove).toHaveBeenCalledWith(pending));
  });

  it('offers no automatic rewrite when the fix is manual', async () => {
    unsafeSteps.mockResolvedValue([manual]);
    render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    await screen.findByText('heredoc');
    expect(screen.queryByText('wf.unsafeSuggest')).toBeNull();
    fireEvent.click(screen.getByText('wf.unsafeHow'));
    expect(screen.getByText('wf.unsafeManual')).toBeDefined();
    expect(screen.getByText(manual.manual_fix!)).toBeDefined();
    expect(screen.queryByText('wf.unsafeApply')).toBeNull();
  });

  it('names the CollectApiData source of an inline Quick Exec', async () => {
    unsafeSteps.mockResolvedValue([{ ...fixable, step_name: 'collect', phase: 'source', source_alias: 'ticket' }]);
    const onApply = vi.fn().mockResolvedValue(undefined);
    render(<UnsafeStepsPanel workflow={workflow} onApply={onApply} />);
    await screen.findByText('collect');
    expect(screen.getByText(/wf\.unsafeSource/)).toBeDefined();
    fireEvent.click(screen.getByText('wf.unsafeSuggest'));
    fireEvent.click(screen.getByText('wf.unsafeApply'));
    await waitFor(() => expect(onApply).toHaveBeenCalledWith(expect.objectContaining({ source_alias: 'ticket' })));
  });

  it('offers a prompt for an agent listing every step that needs a manual fix', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    const second = { ...manual, step_name: 'write_ctx', phase: 'stdin' as const, placeholder: '{{steps.whoami.data.login}}' };
    unsafeSteps.mockResolvedValue([fixable, manual, second]);
    render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);

    const prompt = await screen.findByLabelText('wf.unsafeAgentPromptLabel');
    const text = prompt.textContent ?? '';
    expect(text).toContain('wf.unsafeAgentPrompt|PR review|wf-1|2|');
    expect(text).toContain(`- heredoc : {{issue.title}} — ${manual.manual_fix}`);
    expect(text).toContain('- write_ctx (stdin) : {{steps.whoami.data.login}}');
    expect(text).not.toContain('- greet');

    fireEvent.click(screen.getByText('wf.unsafeAgentCopy'));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(text));
    expect(await screen.findByText('wf.unsafeAgentCopied')).toBeDefined();
  });

  it('offers no agent prompt when every step has an automatic fix', async () => {
    unsafeSteps.mockResolvedValue([fixable]);
    render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    await screen.findByText('greet');
    expect(screen.queryByLabelText('wf.unsafeAgentPromptLabel')).toBeNull();
  });

  it('presents a value inside code as an interpolation, not as an approval', async () => {
    unsafeSteps.mockResolvedValue([fixable]);
    render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    await screen.findByText('greet');
    expect(screen.getByRole('heading').textContent).toContain('wf.unsafeTitle');
    expect(screen.getByText('wf.unsafeIntro')).toBeDefined();
    expect(screen.queryByText('wf.approvalIntro')).toBeNull();
  });

  it('names blocked sub-workflows and rollbacks even when this workflow has none', async () => {
    const blocker = (workflow_id: string, workflow_name: string, human_only: boolean): WorkflowBlocker => ({
      workflow_id, workflow_name, step: 's', on_failure: workflow_name === 'WF2', kind: human_only ? 'human_approval' : 'missing_child',
      message: 'm', action: 'a', human_only,
    });
    const verdict: WorkflowReadiness = {
      workflow_id: 'wf-1', workflow_name: 'PR review', enabled: true, ready: false,
      blockers: [blocker('wf-2', 'WF2', true), blocker('wf-2', 'WF2', true), blocker('wf-3', 'WF3', false)],
      human_approval_count: 2, checked_workflow_ids: ['wf-1', 'wf-2', 'wf-3'], summary: 'NOT READY',
    };
    unsafeSteps.mockResolvedValue([]);
    readiness.mockResolvedValue(verdict);
    render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    const chain = await screen.findByTestId('readiness-chain');
    expect(chain.textContent).toContain('wf.readinessChildItem|WF2|2|2');
    expect(chain.textContent).toContain('wf.readinessChildItem|WF3|1|0');
    expect(readiness).toHaveBeenCalledWith('wf-1');
  });

  it('stays hidden when the chain is ready', async () => {
    unsafeSteps.mockResolvedValue([]);
    readiness.mockResolvedValue({
      workflow_id: 'wf-1', workflow_name: 'PR review', enabled: true, ready: true, blockers: [],
      human_approval_count: 0, checked_workflow_ids: ['wf-1'], summary: 'READY',
    });
    const { container } = render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    await waitFor(() => expect(readiness).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });

  it("shows this workflow's own blockers the unsafe list does not cover, once", async () => {
    const script: WorkflowBlocker = {
      workflow_id: 'wf-1', workflow_name: 'PR review', step: 'run_tool', on_failure: false,
      kind: 'human_approval', phase: 'script', message: '`tool.py` has no approved hash yet.',
      action: 'Ask a human to open the step and save it.', human_only: true,
    };
    const duplicate: WorkflowBlocker = {
      workflow_id: 'wf-1', workflow_name: 'PR review', step: 'greet', on_failure: false,
      kind: 'unsafe_interpolation', phase: 'main', reason: 'inline_code_interpolation',
      message: 'dup', action: 'Rewrite', human_only: false,
    };
    unsafeSteps.mockResolvedValue([]);
    readiness.mockResolvedValue({
      workflow_id: 'wf-1', workflow_name: 'PR review', enabled: true, ready: false,
      blockers: [script], human_approval_count: 1, checked_workflow_ids: ['wf-1'], summary: 'NOT READY',
    });
    const { unmount } = render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    const alert = await screen.findByRole('alert');
    expect(alert.getAttribute('aria-label')).toBe('wf.approvalTitle');
    const local = screen.getByTestId('readiness-local');
    expect(local.textContent).toContain('run_tool');
    expect(local.textContent).toContain('(script)');
    expect(local.textContent).toContain(script.message);
    expect(local.textContent).toContain(script.action);
    unmount();

    unsafeSteps.mockResolvedValue([fixable]);
    readiness.mockResolvedValue({
      workflow_id: 'wf-1', workflow_name: 'PR review', enabled: true, ready: false,
      blockers: [duplicate], human_approval_count: 0, checked_workflow_ids: ['wf-1'], summary: 'NOT READY',
    });
    render(<UnsafeStepsPanel workflow={workflow} onApply={vi.fn()} />);
    await screen.findByText('greet');
    await waitFor(() => expect(readiness).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId('readiness-local')).toBeNull();
    expect(screen.queryByText('dup')).toBeNull();
  });
});

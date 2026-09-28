import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { WorkflowStepActivityBubble } from '../WorkflowStepActivityBubble';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: Array<string | number>) => (
      args.length > 0 ? `${key} ${args.join(' ')}` : key
    ),
  }),
}));

afterEach(() => cleanup());

describe('WorkflowStepActivityBubble', () => {
  it('shows the provider as a discussion agent with workflow, step, and run provenance', () => {
    render(<WorkflowStepActivityBubble step={{
      run_id: 'run-123456789',
      workflow_id: 'workflow-1',
      workflow_name: 'Implementation',
      step_key: 'orchestrate',
      step_name: 'Orchestrate',
      agent_type: 'ClaudeCode',
      started_at: '2026-09-28T08:00:00Z',
    }} />);

    expect(screen.getByTestId('workflow-step-identity')).toHaveTextContent(
      'Implementation›Orchestratedisc.workflowRun run-1234',
    );
    expect(screen.getByText('disc.workflowStepRunning')).toBeInTheDocument();
    const bubble = screen.getByTestId('workflow-step-running-run-123456789-orchestrate');
    expect(bubble).toHaveTextContent('@claude · disc.targetDiscussionAgent');
    expect(bubble).not.toHaveTextContent('CLI');
  });
});

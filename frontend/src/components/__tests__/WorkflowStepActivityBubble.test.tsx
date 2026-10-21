import { act, cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ActiveWorkflowStep } from '../../types/generated';
import { WorkflowStepActivityBubble } from '../WorkflowStepActivityBubble';
import {
  receiveRunFrame,
  resetLiveRuns,
  type AgentRunProgressFrame,
} from '../../lib/agentRunProgress';

vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    locale: 'en',
    t: (key: string, ...args: Array<string | number>) => (
      args.length > 0 ? `${key} ${args.join(' ')}` : key
    ),
  }),
}));

const step = (extra: Partial<ActiveWorkflowStep> = {}): ActiveWorkflowStep => ({
  run_id: 'run-123456789',
  workflow_id: 'workflow-1',
  workflow_name: 'Implementation',
  step_key: 'orchestrate',
  step_name: 'Orchestrate',
  agent_type: 'ClaudeCode',
  started_at: '2026-09-28T08:00:00Z',
  ...extra,
});

const frame = (
  progress: Partial<AgentRunProgressFrame['progress']> = {},
  overrides: Partial<Omit<AgentRunProgressFrame, 'progress'>> = {},
): AgentRunProgressFrame => ({
  type: 'agent_run_progress',
  discussion_id: 'room-1',
  dispatch_id: null,
  trigger_message_id: null,
  agent_type: 'ClaudeCode',
  run_id: 'progress-1',
  started_at: '2026-09-28T08:00:01Z',
  seq: 1,
  ...overrides,
  progress: {
    phase: 'starting_cli',
    phase_ms: 1_000,
    elapsed_ms: 2_000,
    timeline: [],
    mcp_servers: null,
    activity: [],
    tool_calls: 0,
    silent_ms: 0,
    idle_limit_ms: null,
    stopped: null,
    ...progress,
  },
});

beforeEach(() => resetLiveRuns());
afterEach(() => cleanup());

describe('WorkflowStepActivityBubble', () => {
  it('shows the provider as a discussion agent with workflow, step, and run provenance', () => {
    render(<WorkflowStepActivityBubble step={step()} />);

    expect(screen.getByTestId('workflow-step-identity')).toHaveTextContent(
      'Implementation›Orchestratedisc.workflowRun run-1234',
    );
    expect(screen.getByText('disc.workflowStepRunning')).toBeInTheDocument();
    const bubble = screen.getByTestId('workflow-step-running-run-123456789-orchestrate');
    expect(bubble).toHaveTextContent('@claude · disc.targetDiscussionAgent');
    expect(bubble).not.toHaveTextContent('CLI');
  });

  it('says the workflow is preparing while its agent has not entered the room', () => {
    render(<WorkflowStepActivityBubble step={step({ preparing: true, step_name: 'Clone' })} discussionId="room-1" />);

    const bubble = screen.getByTestId('workflow-step-running-run-123456789-orchestrate');
    expect(bubble).toHaveAttribute('data-preparing', 'true');
    expect(bubble).toHaveTextContent('Clone');
    expect(screen.getByText('disc.workflowStepPreparing')).toBeInTheDocument();
    expect(screen.queryByText('disc.workflowStepRunning')).toBeNull();
  });

  it('keeps the placeholder until the step\'s run reports, then shows its phase and tool calls', () => {
    render(<WorkflowStepActivityBubble
      step={step({ progress_run_id: 'progress-1' })}
      discussionId="room-1"
    />);
    expect(screen.getByText('disc.workflowStepRunning')).toBeInTheDocument();
    expect(screen.queryByTestId('agent-run-progress')).toBeNull();

    act(() => receiveRunFrame(frame()));
    expect(screen.queryByText('disc.workflowStepRunning')).toBeNull();
    expect(screen.getByTestId('agent-run-progress')).toHaveAttribute('data-phase', 'starting_cli');

    act(() => receiveRunFrame(frame({
      phase: 'tool',
      tool_calls: 4,
      activity: [{ category: 'Execute', at: new Date().toISOString() }],
    }, { seq: 2 })));
    expect(screen.getByTestId('agent-run-progress')).toHaveAttribute('data-phase', 'tool');
    expect(screen.getByTestId('run-activity-toggle')).toHaveTextContent('4');
  });

  it('never shows the run of another room or another step', () => {
    render(<WorkflowStepActivityBubble
      step={step({ progress_run_id: 'progress-1' })}
      discussionId="room-1"
    />);
    act(() => {
      receiveRunFrame(frame({}, { discussion_id: 'room-2' }));
      receiveRunFrame(frame({}, { run_id: 'progress-other' }));
    });
    expect(screen.queryByTestId('agent-run-progress')).toBeNull();
  });
});

import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';
import { TaskExecutionCard } from '../TaskExecutionCard';
import { allowedActions } from '../../lib/orch-actions';
import type { TaskExecutionDetail, TaskExecutionStatus, TaskExecutionUsage } from '../../types/generated';

const wrap = (ui: React.ReactElement) => render(<I18nProvider>{ui}</I18nProvider>);

const usage = (over: Partial<TaskExecutionUsage> = {}): TaskExecutionUsage => ({
  duration_ms: 42_000,
  in_app_tokens: 0,
  in_app_messages: 0,
  in_app_cost_usd: null,
  in_app_cost_is_partial: false,
  cli_traffic_tokens: 1000,
  cli_billable_tokens: 900,
  cli_sessions: 1,
  cli_sessions_measured: 1,
  cli_sessions_unmeasured: 0,
  http: null,
  ...over,
});

const detail = (over: Partial<TaskExecutionDetail> = {}): TaskExecutionDetail =>
  ({
    lineage: {
      execution: {
        id: 'exec-1',
        status: 'Working' as TaskExecutionStatus,
        worker_agent_type: 'Ollama',
        worker_model: 'qwen3-coder:30b',
        child_branch: 'kronn/task/kt-1',
        candidate_merge_sha: 'abc123',
        integrated_sha: null,
      },
      task_reference: 'KT-1',
      task_title: 'Do the thing',
      parent_discussion_id: 'parent-1',
      sub_discussion_id: 'child-1',
      workspace_canonical_path: '/tmp/wt',
    },
    target_branch: 'main',
    definition_of_done: [],
    attempts: [],
    validation_runs: [],
    recovery: null,
    usage: usage(),
    ...over,
  }) as unknown as TaskExecutionDetail;

describe('allowedActions', () => {
  it('offers review only while a review is actually owed', () => {
    // An action offered in the wrong state is a promise the backend will refuse,
    // and the user only finds out after clicking.
    expect(allowedActions('AwaitingReview').approve).toBe(true);
    expect(allowedActions('Working').approve).toBe(false);
    expect(allowedActions('Done').approve).toBe(false);
  });

  it('never offers to stop something already finished', () => {
    for (const terminal of ['Done', 'Failed', 'Cancelled'] as TaskExecutionStatus[]) {
      expect(allowedActions(terminal).stop).toBe(false);
    }
    expect(allowedActions('Working').stop).toBe(true);
  });

  it('offers reassignment only in backend-resumable worker states', () => {
    expect(allowedActions('Working').reassign).toBe(true);
    expect(allowedActions('ChangesRequested').reassign).toBe(true);
    expect(allowedActions('Interrupted', 'Working').reassign).toBe(true);
    expect(allowedActions('Interrupted', 'AwaitingReview').reassign).toBe(false);
    for (const policyHold of ['Blocked', 'Escalated'] as TaskExecutionStatus[]) {
      expect(allowedActions(policyHold).reassign).toBe(false);
    }
  });
});

describe('TaskExecutionCard', () => {
  it('says a token total is incomplete instead of presenting it as the cost', () => {
    // Sessions that never reported are unknown, not zero. A bare total drawn
    // over them reads as "this is what it cost".
    wrap(
      <TaskExecutionCard
        detail={detail({ usage: usage({ cli_sessions: 3, cli_sessions_measured: 1, cli_sessions_unmeasured: 2 }) })}
      />
    );
    const partial = screen.getByTestId('orch-exec-tokens-partial').textContent ?? '';
    expect(partial).toContain('2');
    expect(partial).toContain('3');
  });

  it('stays quiet about completeness when every session reported', () => {
    wrap(<TaskExecutionCard detail={detail()} />);
    expect(screen.queryByTestId('orch-exec-tokens-partial')).toBeNull();
  });

  it('keeps the selected model and candidate SHA inspectable', () => {
    wrap(<TaskExecutionCard detail={detail()} />);
    expect(screen.getByTestId('orch-exec-model')).toHaveTextContent('qwen3-coder:30b');
    expect(screen.getByTestId('orch-exec-candidate')).toHaveTextContent('abc123');
  });

  it('does not turn an unknown cost into a fake zero', () => {
    wrap(<TaskExecutionCard detail={detail({ usage: usage({ in_app_cost_usd: null }) })} />);
    expect(screen.getByTestId('orch-exec-cost')).toHaveTextContent('—');
  });

  it('shows an em dash rather than zero when the tokens are unknown', () => {
    wrap(
      <TaskExecutionCard
        detail={detail({ usage: usage({ cli_billable_tokens: null, cli_sessions_unmeasured: 1 }) })}
      />
    );
    expect(screen.getByTestId('orch-exec-tokens').textContent).toContain('—');
    expect(screen.getByTestId('orch-exec-tokens').textContent).not.toContain('0 ');
  });

  it('lists the validations that failed, not the ones that passed', () => {
    wrap(
      <TaskExecutionCard
        detail={detail({
          validation_runs: [
            { id: 'v1', command: 'cargo test', exit_code: 0 },
            { id: 'v2', command: 'cargo clippy', exit_code: 101 },
          ] as unknown as TaskExecutionDetail['validation_runs'],
        })}
      />
    );
    const text = screen.getByTestId('orch-exec-failing').textContent ?? '';
    expect(text).toContain('cargo clippy');
    expect(text).not.toContain('cargo test');
  });

  it('renders nothing before the detail is loaded', () => {
    wrap(<TaskExecutionCard detail={null} />);
    expect(screen.queryByTestId('orch-exec-card')).toBeNull();
  });

  it('hides the review controls on a running execution', () => {
    wrap(<TaskExecutionCard detail={detail()} onApprove={vi.fn()} onRequestChanges={vi.fn()} onStop={vi.fn()} />);
    expect(screen.queryByTestId('orch-exec-approve')).toBeNull();
    expect(screen.getByTestId('orch-exec-stop')).toBeTruthy();
  });
});

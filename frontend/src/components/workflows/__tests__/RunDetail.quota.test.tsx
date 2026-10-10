// KT-811 — a run waiting for a provider quota reset reads "quota", never
// "failed", wherever it is shown, and can be resumed or cancelled by hand.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { WorkflowRun, StepResult, SharedRun } from '../../../types/generated';
import { workflowRunStatusCardModel, sharedRunStatusCardModel } from '../../../lib/runStatusCardModel';
import { runMatchesStatusFilter } from '../../../lib/runFilters';
import { dictionaries } from '../../../lib/i18n/testing';

vi.mock('../../../lib/api', () => buildApiMock());

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t }),
}));

import { RunDetail } from '../RunDetail';
import { LiveFinishedBanner } from '../WorkflowDetail';

const step = (over: Partial<StepResult>): StepResult => ({
  step_name: 'prepare',
  status: 'Success',
  output: 'done',
  tokens_used: 0,
  duration_ms: 600,
  is_rollback: false,
  ...over,
});

const WAKE = '2026-09-25T19:42:00Z';
const RESET = '2026-09-25T19:40:00Z';

const waitingRun = (quota: StepResult['quota_wait']): WorkflowRun => ({
  id: 'run-quota',
  workflow_id: 'wf-1',
  status: 'WaitingQuota',
  trigger_context: null,
  step_results: [
    step({}),
    step({
      step_name: 'opus',
      status: 'WaitingQuota',
      output: "Provider quota or session limit: You've hit your session limit",
      quota_wait: quota,
    }),
  ],
  tokens_used: 0,
  workspace_path: null,
  started_at: '2026-09-25T17:00:00Z',
  finished_at: null,
  run_type: 'linear', batch_total: 0, batch_completed: 0, batch_failed: 0, batch_no_response: 0,
  batch_name: null, parent_run_id: null, state: {}, produced_branches: [],
});

describe('a run waiting for a provider quota', () => {
  it('shows when it resumes, the announced reset, and a quota badge on the step', () => {
    render(
      <RunDetail
        run={waitingRun({ reset_at: RESET, wake_at: WAKE, attempt: 1 })}
        onDelete={() => {}}
      />,
    );
    const notice = screen.getByTestId('wf-quota-notice');
    expect(notice.textContent).toContain(`wf.quota.waitingUntil:${new Date(WAKE).toLocaleString()}`);
    expect(notice.textContent).toContain(`wf.quota.reset:${new Date(RESET).toLocaleString()}`);
    expect(notice.dataset.parked).toBeUndefined();
    expect(screen.getAllByText(/run\.status\.quota/).length).toBeGreaterThan(0);
    expect(screen.queryByText(/run\.status\.failed/)).toBeNull();
  });

  it('parked without a reset, it says so and offers Resume and Cancel', () => {
    const onResume = vi.fn();
    const onCancel = vi.fn();
    vi.stubGlobal('confirm', vi.fn(() => true));
    render(
      <RunDetail
        run={waitingRun({ attempt: 1, parked: 'no_reset_time' })}
        onDelete={() => {}}
        onResume={onResume}
        onCancel={onCancel}
      />,
    );
    const notice = screen.getByTestId('wf-quota-notice');
    expect(notice.dataset.parked).toBe('true');
    expect(notice.textContent).toContain('wf.quota.parked.noResetTime');
    const resume = screen.getByTitle('wf.quota.resumeHint');
    fireEvent.click(resume);
    expect(onResume).toHaveBeenCalledWith(false);
    fireEvent.click(screen.getByTitle('wf.cancelRun'));
    expect(onCancel).toHaveBeenCalled();
  });

  it('names each parking reason', () => {
    const cases: Array<[NonNullable<StepResult['quota_wait']>, string]> = [
      [{ attempt: 1, parked: 'after_deadline', reset_at: RESET }, 'wf.quota.parked.afterDeadline'],
      [{ attempt: 5, parked: 'too_many_attempts', reset_at: RESET }, 'wf.quota.parked.tooManyAttempts:4'],
      [{ attempt: 1, parked: 'not_resumable', detail: 'the workflow is disabled' }, 'wf.quota.parked.notResumable:the workflow is disabled'],
    ];
    for (const [wait, expected] of cases) {
      const { unmount } = render(<RunDetail run={waitingRun(wait)} onDelete={() => {}} />);
      expect(screen.getByTestId('wf-quota-notice').textContent).toContain(expected);
      unmount();
    }
  });

  it('a failed child step refused for quota still carries the quota badge', () => {
    const run = waitingRun(null);
    run.status = 'Failed';
    run.finished_at = '2026-09-25T17:05:00Z';
    run.step_results[1] = { ...run.step_results[1], status: 'Failed', quota_wait: { attempt: 0, reset_at: RESET } };
    render(<RunDetail run={run} onDelete={() => {}} />);
    expect(screen.queryByTestId('wf-quota-notice')).toBeNull();
    expect(screen.getByTitle('wf.quota.stepHint').textContent).toContain('run.status.quota');
  });

  it('reads as quota on the status cards and the live banner, and in the waiting filter', () => {
    expect(workflowRunStatusCardModel(waitingRun({ attempt: 1, wake_at: WAKE })).status).toBe('quota');
    const shared = {
      id: 'run-quota', kind: 'workflow', source_id: 'wf-1', project_id: null, discussion_id: null,
      status: 'running', started_at: null, finished_at: null, duration_ms: null,
      result: { progress: { completed: 1, total: 2 }, quota: { attempt: 1, wake_at: WAKE } },
      diagnostic: null, created_at: '', updated_at: '',
    } as unknown as SharedRun;
    expect(sharedRunStatusCardModel(shared).status).toBe('quota');
    expect(sharedRunStatusCardModel({ ...shared, result: { progress: null } } as SharedRun).status).toBe('running');
    expect(runMatchesStatusFilter(waitingRun({ attempt: 1 }), 'waiting')).toBe(true);
    expect(runMatchesStatusFilter(waitingRun({ attempt: 1 }), 'failed')).toBe(false);

    const { container } = render(<LiveFinishedBanner status="WaitingQuota" stepsExecuted={2} t={t} />);
    expect(container.querySelector('.wf-live-finished')?.getAttribute('data-status')).toBe('waiting');
    expect(container.textContent).toContain('run.status.quota');
  });

  it('is labelled quota, not failure, in every locale', () => {
    const failure: Record<string, string> = { en: 'Failed', fr: 'Échec', es: 'Fallido', zh: '失败' };
    for (const [name, locale] of (['en', 'fr', 'es', 'zh'] as const).map(code => [code, dictionaries[code] as Record<string, string>] as const)) {
      const label = locale['run.status.quota'];
      expect(label, name).toBeTruthy();
      expect(label).not.toBe(locale['run.status.failed']);
      expect(label.toLowerCase()).not.toContain(failure[name].toLowerCase());
      for (const key of [
        'wf.quota.waitingUntil', 'wf.quota.reset', 'wf.quota.parked.noResetTime',
        'wf.quota.parked.afterDeadline', 'wf.quota.parked.tooManyAttempts',
        'wf.quota.parked.notResumable', 'wf.quota.resumeHint', 'wf.quota.stepHint',
      ]) {
        expect(locale[key], `${name}:${key}`).toBeTruthy();
      }
    }
  });
});

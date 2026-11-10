// KT-1524 — a run that stopped on a refused token says which connection
// refused it, at run level and in the failed step, not only in the raw output.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { WorkflowRun, StepResult } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());

import { RunDetail } from '../RunDetail';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t }),
}));

const SENTENCE = 'Connection "GitHub Euronews" (GitHub) refused the token (HTTP 401): Bad credentials — update the token in Settings → APIs. (15 of 22 sources)';
const RAW = Array.from({ length: 200 }, (_, i) => `HTTP 401 on GET https://api.github.com/repos/r${i} — {}`).join('\n');

const mkResult = (over: Partial<StepResult>): StepResult => ({
  step_name: 'collect',
  status: 'Failed',
  output: `${SENTENCE}\n${RAW}\n[SIGNAL: auth_refused]`,
  tokens_used: 0,
  duration_ms: 600,
  is_rollback: false,
  ...over,
});

const mkRun = (steps: StepResult[]): WorkflowRun => ({
  id: 'run-1',
  workflow_id: 'wf-1',
  status: 'Failed',
  trigger_context: null,
  step_results: steps,
  tokens_used: 0,
  workspace_path: null,
  started_at: '2026-11-10T08:00:00Z',
  finished_at: '2026-11-10T08:00:01Z',
  run_type: 'linear', batch_total: 0, batch_completed: 0, batch_failed: 0, batch_no_response: 0,
  batch_name: null, parent_run_id: null, state: {}, produced_branches: [],
});

describe('RunDetail — refused token', () => {
  it('names the connection at run level, above the step list', () => {
    render(<RunDetail run={mkRun([mkResult({})])} onDelete={() => {}} />);

    const notice = screen.getByTestId('wf-auth-refused');
    expect(within(notice).getByText('wf.authRefused.title')).toBeInTheDocument();
    expect(notice.textContent).toContain('wf.authRefused.step:collect');
    expect(notice.textContent).toContain(SENTENCE);
    expect(within(notice).getByText('wf.authRefused.hint')).toBeInTheDocument();
  });

  it('leads the expanded failed step with the sentence', () => {
    render(<RunDetail run={mkRun([mkResult({})])} onDelete={() => {}} />);
    fireEvent.click(screen.getByText('collect'));

    expect(screen.getByTestId('wf-step-auth-refused').textContent).toBe(SENTENCE);
  });

  it('says nothing for a run that failed for another reason', () => {
    render(<RunDetail run={mkRun([mkResult({ output: 'HTTP 500 on GET x — boom' })])} onDelete={() => {}} />);
    expect(screen.queryByTestId('wf-auth-refused')).toBeNull();
  });
});

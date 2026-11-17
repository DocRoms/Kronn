// KT-1796 — a possible secret fails no step, so the run says it at run level:
// which step, which document, which lines, and that nothing was blocked.

import { describe, it, expect, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { WorkflowRun } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());

import { RunDetail } from '../RunDetail';

const t = (key: string, ...args: (string | number)[]) =>
  args.length > 0 ? `${key}:${args.join(',')}` : key;

vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t }),
}));

const mkRun = (state: Record<string, string>): WorkflowRun => ({
  id: 'run-1',
  workflow_id: 'wf-1',
  status: 'Success',
  trigger_context: null,
  step_results: [{
    step_name: 'advise', status: 'Success', output: 'done', tokens_used: 0, duration_ms: 600, is_rollback: false,
  }],
  tokens_used: 0,
  workspace_path: null,
  started_at: '2026-11-17T08:00:00Z',
  finished_at: '2026-11-17T08:00:01Z',
  run_type: 'linear', batch_total: 0, batch_completed: 0, batch_failed: 0, batch_no_response: 0,
  batch_name: null, parent_run_id: null, state, produced_branches: [],
});

describe('RunDetail — possible secrets', () => {
  it('names the step, the document and its lines on a successful run', () => {
    const report = JSON.stringify({
      documents: [
        { path: 'docs/notes.md', lines: [2, 4], more_lines: 3 },
        { path: 'docs/other.md', lines: [7], more_lines: 0 },
      ],
      more_documents: 6,
    });
    render(<RunDetail run={mkRun({ '__kronn.docs_possible_secrets.advise': report })} onDelete={() => {}} />);

    const notice = screen.getByTestId('wf-possible-secrets');
    expect(within(notice).getByText('wf.possibleSecrets.title')).toBeInTheDocument();
    expect(notice.textContent).toContain('wf.possibleSecrets.step:advise');
    expect(notice.textContent).toContain('wf.possibleSecrets.lines:docs/notes.md,2, 4');
    expect(notice.textContent).toContain('wf.possibleSecrets.moreLines:3');
    expect(notice.textContent).toContain('wf.possibleSecrets.lines:docs/other.md,7');
    expect(notice.textContent).toContain('wf.possibleSecrets.moreDocuments:6');
    expect(within(notice).getByText('wf.possibleSecrets.hint')).toBeInTheDocument();
  });

  it('still says it when the record cannot be read', () => {
    render(<RunDetail run={mkRun({ '__kronn.docs_possible_secrets.advise': 'damaged' })} onDelete={() => {}} />);
    const notice = screen.getByTestId('wf-possible-secrets');
    expect(notice.textContent).toContain('wf.possibleSecrets.step:advise');
    expect(within(notice).getByText('wf.possibleSecrets.unreadable')).toBeInTheDocument();
  });

  it('says nothing for a run without any', () => {
    render(<RunDetail run={mkRun({})} onDelete={() => {}} />);
    expect(screen.queryByTestId('wf-possible-secrets')).toBeNull();
  });
});

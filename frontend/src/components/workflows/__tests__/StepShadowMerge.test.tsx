// KT-1544 — the optional merge of a step's main execution with its shadows: the
// step editor (off by default, which shadows merge, wait, condense agent) and
// the run view (each execution's contribution, the provenance of each point,
// the merged result against each execution).
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within, act } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import type {
  ShadowExecution, ShadowMerge, ShadowMergeReport, StepShadow, StepShadowComparison, WorkflowStep,
} from '../../../types/generated';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

import { StepShadowsEditor } from '../StepShadowsEditor';
import { StepShadowCompare } from '../StepShadowCompare';
import { mergeExecutionLabel, mergeInProgress, pruneMerge } from '../../../lib/shadowRuns';
import { workflows as workflowsApi } from '../../../lib/api';

const t = (key: string, ..._args: (string | number)[]) => key;

beforeEach(() => { vi.clearAllMocks(); });

const shadow = (id: string, extra: Partial<StepShadow> = {}): StepShadow => ({ id, label: '', enabled: true, ...extra });
const step = (extra: Partial<WorkflowStep> = {}): WorkflowStep => ({ name: 'review', skill_ids: [], ...extra }) as WorkflowStep;
const merge = (extra: Partial<ShadowMerge> = {}): ShadowMerge => ({ enabled: true, ...extra });

describe('StepShadowsEditor — the merge of a step (KT-1544)', () => {
  const renderEditor = (target: WorkflowStep, onChange = vi.fn()) => {
    render(
      <I18nProvider>
        <StepShadowsEditor step={target} onChange={onChange} projects={[]} skills={[]} quickPrompts={[]} t={t} />
      </I18nProvider>,
    );
    return onChange;
  };
  const two = [shadow('local'), shadow('other', { label: 'Other model' })];

  it('is off by default: a step with shadows only compares them', () => {
    const onChange = renderEditor(step({ shadows: two }));
    const toggle = screen.getByTestId('wf-step-shadows-merge-enabled') as HTMLInputElement;
    expect(toggle.checked).toBe(false);
    expect(screen.getByTestId('wf-step-shadows-merge-help')).toHaveTextContent('wf.shadow.merge.off');
    expect(screen.queryByTestId('wf-step-shadows-merge-which')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('turns the merge on with every shadow, then off again', () => {
    const onChange = renderEditor(step({ shadows: two }));
    fireEvent.click(screen.getByTestId('wf-step-shadows-merge-enabled'));
    expect(onChange).toHaveBeenLastCalledWith(two, { enabled: true });
  });

  it('turns an enabled merge off', () => {
    const onChange = renderEditor(step({ shadows: two, shadow_merge: merge({ shadows: ['local'] }) }));
    expect(screen.getByTestId('wf-step-shadows-merge-help')).toHaveTextContent('wf.shadow.merge.description');
    fireEvent.click(screen.getByTestId('wf-step-shadows-merge-enabled'));
    expect(onChange).toHaveBeenLastCalledWith(two, null);
  });

  it('picks one shadow, several, or all of them', () => {
    const onChange = renderEditor(step({ shadows: [...two, shadow('third')], shadow_merge: merge() }));
    // « All » is the starting point: every shadow reads as selected.
    expect(screen.getByTestId('wf-step-shadows-merge-all')).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('wf-step-shadows-merge-shadow-other')).toHaveTextContent('Other model');
    fireEvent.click(screen.getByTestId('wf-step-shadows-merge-shadow-local'));
    expect(onChange).toHaveBeenLastCalledWith(expect.anything(), { enabled: true, shadows: ['other', 'third'] });
  });

  it('selects one shadow and goes back to all when every one is picked', () => {
    const only = [shadow('local'), shadow('other')];
    const { unmount } = render(
      <I18nProvider>
        <StepShadowsEditor step={step({ shadows: only, shadow_merge: merge({ shadows: ['other'] }) })} onChange={vi.fn()} projects={[]} skills={[]} quickPrompts={[]} t={t} />
      </I18nProvider>,
    );
    expect(screen.getByTestId('wf-step-shadows-merge-shadow-other')).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByTestId('wf-step-shadows-merge-shadow-local')).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByTestId('wf-step-shadows-merge-all')).toHaveAttribute('aria-pressed', 'false');
    unmount();
    const onChange = vi.fn();
    render(
      <I18nProvider>
        <StepShadowsEditor step={step({ shadows: only, shadow_merge: merge({ shadows: ['other'] }) })} onChange={onChange} projects={[]} skills={[]} quickPrompts={[]} t={t} />
      </I18nProvider>,
    );
    fireEvent.click(screen.getByTestId('wf-step-shadows-merge-shadow-local'));
    expect(onChange).toHaveBeenLastCalledWith(only, { enabled: true, shadows: [] });
  });

  it('never leaves the merge with no shadow selected', () => {
    const onChange = renderEditor(step({ shadows: two, shadow_merge: merge({ shadows: ['local'] }) }));
    fireEvent.click(screen.getByTestId('wf-step-shadows-merge-shadow-local'));
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId('wf-step-shadows-merge-all'));
    expect(onChange).toHaveBeenLastCalledWith(two, { enabled: true, shadows: [] });
  });

  it('sets how long the merge waits, in minutes, and the condense agent', () => {
    const onChange = renderEditor(step({ shadows: two, shadow_merge: merge() }));
    fireEvent.change(screen.getByTestId('wf-step-shadows-merge-timeout'), { target: { value: '15' } });
    expect(onChange).toHaveBeenLastCalledWith(two, { enabled: true, timeout_secs: 900 });
    fireEvent.change(screen.getByTestId('wf-step-shadows-merge-condense-agent'), { target: { value: 'ClaudeCode' } });
    expect(onChange).toHaveBeenLastCalledWith(two, { enabled: true, agent: { agent: 'ClaudeCode' } });
  });

  it('shows the saved wait and condense agent', () => {
    renderEditor(step({ shadows: two, shadow_merge: merge({ timeout_secs: 1200, agent: { agent: 'ClaudeCode', model: 'opus' } }) }));
    expect(screen.getByTestId('wf-step-shadows-merge-timeout')).toHaveValue(20);
    expect(screen.getByTestId('wf-step-shadows-merge-condense-agent')).toHaveValue('ClaudeCode');
    expect(screen.getByTestId('wf-step-shadows-merge-condense-model')).toHaveValue('opus');
  });

  it('follows a renamed shadow and forgets a removed one, so a save is never refused', () => {
    const onChange = renderEditor(step({ shadows: two, shadow_merge: merge({ shadows: ['local'] }) }));
    fireEvent.change(screen.getByTestId('wf-shadow-local-id'), { target: { value: 'qwen' } });
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen'), two[1]], { enabled: true, shadows: ['qwen'] });
    fireEvent.click(screen.getByTestId('wf-shadow-local-remove'));
    expect(onChange).toHaveBeenLastCalledWith([two[1]], { enabled: true, shadows: [] });
  });

  it('drops the merge with the last shadow', () => {
    const onChange = renderEditor(step({ shadows: [shadow('only')], shadow_merge: merge() }));
    fireEvent.click(screen.getByTestId('wf-shadow-only-remove'));
    expect(onChange).toHaveBeenLastCalledWith([], null);
  });

  it('offers no merge on a step without shadows or one that acts outside Kronn', () => {
    renderEditor(step({ shadows: [], external_effect: true }));
    expect(screen.queryByTestId('wf-step-shadows-merge')).toBeNull();
  });

  it('leaves a merge that names only existing shadows as it is', () => {
    expect(pruneMerge(merge({ shadows: ['a'] }), [shadow('a'), shadow('b')], [shadow('a'), shadow('b')])).toBeUndefined();
    expect(pruneMerge(undefined, [shadow('a')], [])).toBeUndefined();
    expect(pruneMerge(merge(), [shadow('a'), shadow('b')], [shadow('a')])).toBeUndefined();
  });
});

const exec = (shadowId: string | null, extra: Partial<ShadowExecution> = {}): ShadowExecution => ({
  shadow_id: shadowId ?? undefined,
  label: shadowId ?? '',
  status: 'success',
  identity: { agent: 'ClaudeCode', model: 'opus', prompt: 'qp-review@13', skills: ['rust'] },
  output: 'MAIN: SQL injection',
  tokens_used: 100,
  duration_ms: 1000,
  cost_usd: 0.1,
  model: 'opus',
  html: 'none',
  ...extra,
});

const report = (extra: Partial<ShadowMergeReport> = {}): ShadowMergeReport => ({
  status: 'merged',
  selected: ['local', 'other'],
  contributors: ['local', 'other'],
  missing: [],
  timeout_secs: 600,
  points: [
    { id: 'p1', summary: 'SQL injection', found_by: ['main', 'shadow:local'], verdict: 'kept', verified: true, evidence: 'login.rs:12' },
    { id: 'p2', summary: 'cache leaks tenants', found_by: ['shadow:local'], verdict: 'dropped_unverified', verified: false, reason: 'no such cache' },
    { id: 'p3', summary: 'Surrogate-Capability', found_by: ['shadow:other'], verdict: 'kept', verified: true, evidence: 'edge.rs:7' },
  ],
  contributions: [
    { execution: 'main', kept: 1, unique: 0, duplicate: 0, dropped_unverified: 0 },
    { execution: 'shadow:local', kept: 0, unique: 0, duplicate: 1, dropped_unverified: 1 },
    { execution: 'shadow:other', kept: 1, unique: 1, duplicate: 0, dropped_unverified: 0 },
  ],
  merged_output: 'MERGED: SQL injection\nSurrogate-Capability',
  tokens_used: 1500,
  cost_usd: 0.3,
  model: 'opus',
  ...extra,
});

const merged = (extra: Partial<ShadowMergeReport> = {}, shadows?: ShadowExecution[]): StepShadowComparison => ({
  step_name: 'review',
  result_index: 1,
  main: exec(null),
  shadows: shadows ?? [
    exec('local', { label: 'Local qwen', output: 'LOCAL: SQL injection\nINVENTED: cache leaks tenants' }),
    exec('other', { label: 'Other model', output: 'OTHER: Surrogate-Capability' }),
  ],
  merge: report(extra),
});

describe('StepShadowCompare — the merged step (KT-1544)', () => {
  const renderCompare = () => render(<I18nProvider><StepShadowCompare workflowId="wf" runId="r1" runStatus="Success" /></I18nProvider>);
  afterEach(() => { vi.useRealTimers(); });

  it('shows what each execution brought: raised, in the result, found alone, left out', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([merged()]);
    renderCompare();
    const table = await screen.findByTestId('wf-shadow-merge-contrib-review');
    expect(table).toBeInTheDocument();
    expect(screen.getByTestId('wf-shadow-merge-review')).toHaveAttribute('data-status', 'merged');
    const cells = (id: string) => {
      const row = screen.getByTestId(`wf-shadow-merge-contrib-review-${id}`);
      return ['raised', 'kept', 'unique', 'dropped'].map(col => row.querySelector(`[data-col="${col}"]`)?.textContent);
    };
    // p1 is found by main and local: both have it in the result, neither alone.
    expect(cells('main')).toEqual(['1', '1', '0', '0']);
    expect(cells('shadow:local')).toEqual(['2', '1', '0', '1']);
    expect(cells('shadow:other')).toEqual(['1', '1', '1', '0']);
    const local = screen.getByTestId('wf-shadow-merge-contrib-review-shadow:local');
    expect(local.querySelector('[data-col="dropped"]')).toHaveAttribute('data-warn', 'true');
    // What a shadow found that nobody else did is the reason it ran.
    expect(screen.getByTestId('wf-shadow-merge-contrib-review-shadow:other').querySelector('[data-col="unique"]')).toHaveAttribute('data-brought', 'true');
    expect(screen.getByTestId('wf-shadow-merge-tally-review')).toHaveTextContent('2');
  });

  it('names where each point came from and why a dropped point was dropped', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([merged()]);
    renderCompare();
    const points = await screen.findByTestId('wf-shadow-merge-points-review');
    fireEvent.click(points.querySelector('summary')!);
    const dropped = screen.getByTestId('wf-shadow-merge-point-review-p2');
    expect(dropped).toHaveAttribute('data-verdict', 'dropped_unverified');
    expect(dropped.textContent).toContain('cache leaks tenants');
    expect(dropped.textContent).toContain('no such cache');
    expect(dropped.textContent).toContain('Local qwen');
    const kept = screen.getByTestId('wf-shadow-merge-point-review-p1');
    expect(kept).toHaveAttribute('data-verdict', 'kept');
    expect(kept.textContent).toContain('login.rs:12');
  });

  it('says which selected shadows are missing and why', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([
      merged({ contributors: ['other'], missing: [{ shadow_id: 'local', label: 'Local qwen', reason: 'still running when the merge stopped waiting (600 s)' }] }),
    ]);
    renderCompare();
    const missing = await screen.findByTestId('wf-shadow-merge-missing-review-local');
    expect(missing.textContent).toContain('Local qwen');
    expect(missing.textContent).toContain('still running');
  });

  it('sets the merged result against each execution, and says when it is the same', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([merged({ merged_output: 'OTHER: Surrogate-Capability' })]);
    renderCompare();
    const versus = await screen.findByTestId('wf-shadow-merge-vs-review');
    expect(within(versus).getAllByRole('group').length).toBe(3);
    expect(screen.getByTestId('wf-shadow-merge-vs-review-other')).toHaveAttribute('data-same', 'true');
    const local = screen.getByTestId('wf-shadow-merge-vs-review-local');
    expect(local).toHaveAttribute('data-same', 'false');
    fireEvent.click(local.querySelector('summary')!);
    expect(screen.getByTestId('wf-shadow-merge-diff-review-local')).toBeInTheDocument();
    expect(screen.getByTestId('wf-shadow-merge-diff-review-main')).toBeInTheDocument();
  });

  it('reports what the condense pass spent', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([merged()]);
    renderCompare();
    const spent = await screen.findByTestId('wf-shadow-merge-spent-review');
    expect(spent.textContent).toMatch(/1\D500/);
  });

  it('keeps the main output and says why when the merged result was not published', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([
      merged({ status: 'rejected', merged_output: '', note: 'The merged result was not published (point `p2` was dropped as unverified yet appears in the result)' }),
    ]);
    renderCompare();
    const note = await screen.findByTestId('wf-shadow-merge-note-review');
    expect(note.textContent).toContain('not published');
    expect(screen.getByTestId('wf-shadow-merge-review')).toHaveAttribute('data-status', 'rejected');
    // The main execution's output stays the step's output (the page is in French by default).
    expect(screen.getByTestId('wf-shadow-merge-status-review').textContent).toContain('principale');
    expect(screen.queryByTestId('wf-shadow-merge-vs-review')).toBeNull();
  });

  it('keeps refreshing while the merge waits for the shadows', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    vi.mocked(workflowsApi.stepShadows)
      .mockResolvedValueOnce([merged({ status: 'waiting', merged_output: '', points: [], contributions: [], tokens_used: undefined })])
      .mockResolvedValue([merged()]);
    renderCompare();
    await waitFor(() => expect(screen.getByTestId('wf-shadow-merge-review')).toHaveAttribute('data-status', 'waiting'));
    await act(async () => { await vi.advanceTimersByTimeAsync(3100); });
    await waitFor(() => expect(screen.getByTestId('wf-shadow-merge-review')).toHaveAttribute('data-status', 'merged'));
  });

  it('shows no merge for a step that only compares its shadows', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([{ ...merged(), merge: undefined }]);
    renderCompare();
    await screen.findByTestId('wf-step-shadow-compare-review');
    expect(screen.queryByTestId('wf-shadow-merge-review')).toBeNull();
  });
});

describe('merge helpers', () => {
  it('names the executions of a merge, and knows when one is still going', () => {
    const comparison = merged();
    expect(mergeExecutionLabel('main', comparison, 'Main')).toBe('Main (ClaudeCode · opus)');
    expect(mergeExecutionLabel('shadow:local', comparison, 'Main')).toBe('Local qwen (ClaudeCode · opus)');
    expect(mergeExecutionLabel('shadow:ghost', comparison, 'Main')).toBe('ghost');
    expect(mergeInProgress(report({ status: 'waiting' }))).toBe(true);
    expect(mergeInProgress(report({ status: 'condensing' }))).toBe(true);
    expect(mergeInProgress(report())).toBe(false);
    expect(mergeInProgress(undefined)).toBe(false);
  });
});

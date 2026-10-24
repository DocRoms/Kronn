// KT-1449 — the shadow settings card, the run helpers and the side-by-side
// comparison of a normal run and its shadow.
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import type { Project, QuickPrompt, StepResult, WorkflowRun, WorkflowShadow, WorkflowStep } from '../../../types/generated';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

import { ShadowRunsCard } from '../ShadowRunsCard';
import { ShadowCompare } from '../ShadowCompare';
import { clip, compareRuns, isShadowRun, pairedShadows, readableOutput, runListRows, shadowsByNormalRun } from '../../../lib/shadowRuns';
import { skills as skillsApi, workflows as workflowsApi } from '../../../lib/api';

const t = (key: string, ..._args: (string | number)[]) => key;
const run = (id: string, context: unknown, steps: Partial<StepResult>[] = []): WorkflowRun => ({
  id, workflow_id: 'wf', status: 'Success', trigger_context: context, tokens_used: 0, workspace_path: null,
  started_at: '2026-10-24T10:00:00Z', finished_at: null, run_type: 'linear', batch_total: 0, batch_completed: 0,
  batch_failed: 0, batch_no_response: 0, batch_name: null, parent_run_id: null,
  step_results: steps.map(s => ({ step_name: 'x', status: 'Success', output: '', ...s }) as StepResult),
} as WorkflowRun);

beforeEach(() => { vi.clearAllMocks(); });

describe('shadow runs (KT-1449)', () => {
  it('links each shadow run to its normal run', () => {
    const normal = run('n1', {});
    const shadow = run('s1', { __variant__: 'shadow', __shadow_of__: 'n1' });
    expect(isShadowRun(shadow)).toBe(true);
    expect(isShadowRun(normal)).toBe(false);
    expect(shadowsByNormalRun([normal, shadow]).get('n1')?.id).toBe('s1');
  });

  it('folds a paired shadow into its normal row and keeps an orphan shadow', () => {
    const normal = run('n1', {});
    const shadow = run('s1', { __variant__: 'shadow', __shadow_of__: 'n1' });
    const orphan = run('s2', { __variant__: 'shadow', __shadow_of__: 'gone' });
    const runs = [shadow, normal, orphan];
    const pairs = pairedShadows(runs);
    expect([...pairs.keys()]).toEqual(['n1']);
    expect(runListRows(runs, runs, pairs).map(r => r.id)).toEqual(['n1', 's2']);
    // Only the shadow matches: its normal row is still listed.
    expect(runListRows(runs, [shadow], pairs).map(r => r.id)).toEqual(['n1']);
    expect(runListRows(runs, [], pairs)).toEqual([]);
  });

  it('enables the shadow and pins the version of a custom skill the steps use', async () => {
    vi.mocked(skillsApi.history).mockResolvedValue([
      { skill_id: 'custom-review', version_index: 2, name: 'Review', description: '', icon: '', category: 'Domain', content: '', created_at: '', current: true },
      { skill_id: 'custom-review', version_index: 1, name: 'Review', description: '', icon: '', category: 'Domain', content: '', created_at: '', current: false },
    ]);
    const onChange = vi.fn();
    const steps = [{ name: 'revue', skill_ids: ['custom-review', 'rust'] }] as WorkflowStep[];
    const value: WorkflowShadow = { enabled: true };
    render(
      <I18nProvider>
        <ShadowRunsCard value={value} onChange={onChange} steps={steps} projects={[]} skills={[]} quickPrompts={[]} t={t} />
      </I18nProvider>,
    );
    fireEvent.click(screen.getByTestId('wf-shadow-enabled'));
    expect(onChange).toHaveBeenLastCalledWith({ enabled: false });
    const row = screen.getByTestId('wf-shadow-skill-custom-review');
    await waitFor(() => expect(within(row).getAllByRole('option')).toHaveLength(3));
    fireEvent.change(within(row).getByRole('combobox'), { target: { value: '1' } });
    expect(onChange).toHaveBeenLastCalledWith({ enabled: true, skills: { 'custom-review': 'custom-review@1' } });
    expect(screen.queryByTestId('wf-shadow-skill-rust')).toBeNull();
  });

  it('offers a version for the skills a step inherits from its Quick Prompt', async () => {
    vi.mocked(skillsApi.history).mockResolvedValue([]);
    const steps = [{ name: 'revue', skill_ids: [], quick_prompt_id: 'qp-review' }] as unknown as WorkflowStep[];
    const quickPrompts = [{ id: 'qp-review', name: 'Review', skill_ids: ['custom-review'] }] as unknown as QuickPrompt[];
    render(
      <I18nProvider>
        <ShadowRunsCard value={{ enabled: true }} onChange={vi.fn()} steps={steps} projects={[]} skills={[]} quickPrompts={quickPrompts} t={t} />
      </I18nProvider>,
    );
    expect(await screen.findByTestId('wf-shadow-skill-custom-review')).toBeInTheDocument();
  });

  it('says which steps the shadow skips, or warns that it skips none', () => {
    const steps = [
      { name: 'lire', skill_ids: [] },
      { name: 'poster', external_effect: true },
      { name: 'valider', step_type: { type: 'Gate' } },
    ] as unknown as WorkflowStep[];
    const { rerender } = render(
      <I18nProvider><ShadowRunsCard value={{ enabled: true }} onChange={vi.fn()} steps={steps} projects={[]} skills={[]} quickPrompts={[]} t={t} /></I18nProvider>,
    );
    expect(screen.getByTestId('wf-shadow-skipped')).toBeInTheDocument();
    rerender(
      <I18nProvider><ShadowRunsCard value={{ enabled: true }} onChange={vi.fn()} steps={[steps[0]]} projects={[]} skills={[]} quickPrompts={[]} t={t} /></I18nProvider>,
    );
    expect(screen.getByTestId('wf-shadow-nothing-skipped')).toBeInTheDocument();
  });

  it('shows what the normal run loads and flags a shadow version that changes nothing', async () => {
    vi.mocked(skillsApi.history).mockResolvedValue([
      { skill_id: 'custom-review', version_index: 3, name: 'Review', description: '', icon: '', category: 'Domain', content: '', created_at: '', current: true },
      { skill_id: 'custom-review', version_index: 2, name: 'Review', description: '', icon: '', category: 'Domain', content: '', created_at: '', current: false },
    ]);
    const steps = [{ name: 'a', skill_ids: ['custom-review@2'] }, { name: 'b', skill_ids: ['custom-review'] }] as WorkflowStep[];
    const projects = [{ id: 'p1', name: 'Site' }] as Project[];
    render(
      <I18nProvider>
        <ShadowRunsCard value={{ enabled: true, skills: { 'custom-review': 'custom-review@3' } }} onChange={vi.fn()} steps={steps} projects={projects} skills={[]} quickPrompts={[]} t={(key, ...args) => [key, ...args].join(':')} />
      </I18nProvider>,
    );
    const row = screen.getByTestId('wf-shadow-skill-custom-review');
    await waitFor(() => expect(row.textContent).toContain('wf.shadow.latestVersion:3'));
    expect(row.textContent).toContain('wf.shadow.pinnedVersion:2');
    // Step `a` pins v2: v3 still differs from the normal run, so no warning.
    expect(row.textContent).not.toContain('wf.shadow.sameVersionWarning');
    expect(screen.getByRole('button', { name: 'Site' })).toHaveAttribute('aria-pressed', 'false');
  });
});

const step = (name: string, output: string, extra: Partial<StepResult> = {}) => ({ step_name: name, output, status: 'Success', duration_ms: 10, ...extra }) as StepResult;
const envelope = (data: unknown) => `---STEP_OUTPUT---\n${JSON.stringify({ data, status: 'OK' })}\n---END_STEP_OUTPUT---\n[SIGNAL: OK]`;

describe('comparing a run with its shadow (KT-1449)', () => {
  it('gives each step occurrence a verdict, skipped only by the trailing signal', () => {
    const normal = run('n', {}, [step('lire', envelope({ a: 1 })), step('boucle', 'x'), step('boucle', 'y'), step('poster', 'posted'), step('fin', 'ok')]);
    const shadow = run('s', {}, [
      step('lire', envelope({ a: 1 })),
      step('boucle', 'x'),
      step('boucle', 'y2'),
      step('poster', 'Skipped in this shadow run.\n[SIGNAL: SKIPPED_SHADOW]'),
    ]);
    const { steps, counts } = compareRuns(normal, shadow);
    expect(steps.map(s => [s.name, s.occurrence, s.verdict])).toEqual([
      ['lire', 1, 'same'], ['boucle', 1, 'same'], ['boucle', 2, 'different'], ['poster', 1, 'skipped'], ['fin', 1, 'missing'],
    ]);
    expect(counts).toEqual({ same: 2, different: 1, skipped: 1, missing: 1 });
    const quoted = run('q', {}, [step('lire', 'the prompt says [SIGNAL: SKIPPED_SHADOW] then goes on')]);
    expect(compareRuns(normal, quoted).steps[0].verdict).toBe('different');
  });

  it('does not call a step different for its timings alone', () => {
    const exec = (ms: number, stdout: string) => step('jira_key', envelope({ duration_ms: ms, exit_code: 0, stdout, summary: `exit 0 — ${ms} ms` }));
    const normal = run('n', {}, [exec(22, 'NONE')]);
    expect(compareRuns(normal, run('s', {}, [exec(21, 'NONE')])).steps[0].verdict).toBe('same');
    expect(compareRuns(normal, run('s', {}, [exec(21, 'EW-1')])).steps[0].verdict).toBe('different');
  });

  it('pretty-prints JSON envelopes so a diff points at the field', () => {
    expect(readableOutput(envelope({ a: 1, b: 'x' }))).toBe('{\n  "data": {\n    "a": 1,\n    "b": "x"\n  },\n  "status": "OK"\n}');
    expect(readableOutput('plain text\n[SIGNAL: OK]')).toBe('plain text');
    expect(clip('a😀b', 2)).toEqual({ text: 'a😀', clipped: true });
  });

  it('loads both full runs, then shows verdicts, the diff and the totals', async () => {
    const normalFull = run('n1', {}, [step('revue', 'normal verdict', { tokens_used: 100 }), step('envoyer', 'posted')]);
    const shadowFull = run('s1', { __variant__: 'shadow', __shadow_of__: 'n1' }, [
      step('revue', 'shadow verdict', { tokens_used: 80 }),
      step('envoyer', 'Skipped in this shadow run.\n[SIGNAL: SKIPPED_SHADOW]'),
    ]);
    const summary = (r: WorkflowRun) => ({ ...r, step_results: r.step_results.map(s => ({ ...s, output: '' })) });
    vi.mocked(workflowsApi.getRun).mockImplementation(async (_wf, id) => (id === 'n1' ? normalFull : shadowFull));
    render(<I18nProvider><ShadowCompare normal={summary(normalFull)} shadow={summary(shadowFull)} /></I18nProvider>);
    const diff = await screen.findByTestId('wf-shadow-diff-revue');
    expect(diff.textContent).toContain('normal verdict');
    expect(diff.textContent).toContain('shadow verdict');
    expect(screen.getByTestId('wf-shadow-compare-envoyer')).toHaveAttribute('data-verdict', 'skipped');
    expect(screen.getByTestId('wf-shadow-compare-summary').textContent).toContain('100 → 80');
    fireEvent.click(screen.getByRole('checkbox'));
    expect(screen.queryByTestId('wf-shadow-compare-envoyer')).toBeNull();
    expect(screen.getByTestId('wf-shadow-compare-revue')).toBeInTheDocument();
  });
});

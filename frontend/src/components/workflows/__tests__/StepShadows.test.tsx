// KT-1449, KT-1459 — the shadows of a step: the editor on the step, the helpers
// that read what a run recorded, and the side-by-side comparison of the main
// execution and its shadows.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, within, act, cleanup } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import type {
  Project, QuickPrompt, ShadowExecution, StepShadow, StepShadowComparison, WorkflowRun, WorkflowStep,
} from '../../../types/generated';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

import { ShadowStatusButton, StepShadowsEditor } from '../StepShadowsEditor';
import { StepShadowCompare } from '../StepShadowCompare';
import {
  clip, compareIdentities, determinism, hasOutput, isIdenticalShadow, isShadowRun, percent, readableOutput, sameIdentity,
  shadowedSteps, shadowTestedCount,
} from '../../../lib/shadowRuns';
import { skills as skillsApi, workflows as workflowsApi } from '../../../lib/api';

const t = (key: string, ..._args: (string | number)[]) => key;

beforeEach(() => { vi.clearAllMocks(); });

const shadow = (id: string, extra: Partial<StepShadow> = {}): StepShadow => ({ id, label: '', enabled: true, ...extra });
const step = (extra: Partial<WorkflowStep> = {}): WorkflowStep => ({ name: 'review', skill_ids: [], ...extra }) as WorkflowStep;

const skillVersions = (...indexes: number[]) => indexes.map((version_index, i) => ({
  skill_id: 'custom-review', version_index, name: 'Review', description: '', icon: '', category: 'Domain', content: '', created_at: '', current: i === 0,
}));

describe('step shadow helpers', () => {
  it('knows an identical shadow from one that changes something', () => {
    expect(isIdenticalShadow(shadow('a'))).toBe(true);
    expect(isIdenticalShadow(shadow('a', { skills: {} }))).toBe(true);
    expect(isIdenticalShadow(shadow('a', { agent: { agent: 'Codex' } }))).toBe(false);
    expect(isIdenticalShadow(shadow('a', { prompt: { type: 'inline', template: 'x' } }))).toBe(false);
    expect(isIdenticalShadow(shadow('a', { skills: { 'custom-x': '' } }))).toBe(false);
    expect(isIdenticalShadow(shadow('a', { quick_prompt_versions: { q: 2 } }))).toBe(false);
  });

  it('counts what the shadows of the steps test, on the steps that carry some', () => {
    const steps = [
      step({ name: 'a', shadows: [shadow('s1', { skills: { x: 'x@1' }, quick_prompt_versions: { q: 2 } }), shadow('s2', { prompt: { type: 'quick_prompt', id: 'qp-alt' } })] }),
      step({ name: 'b' }),
      step({ name: 'c', shadows: [shadow('s3')] }),
    ];
    expect(shadowTestedCount(steps)).toBe(3);
    expect(shadowedSteps(steps).map(entry => [entry.step.name, entry.shadows.length])).toEqual([['a', 2], ['c', 1]]);
    expect(shadowTestedCount(null)).toBe(0);
  });

  it('flags a run of the former workflow-level shadow, never a normal one', () => {
    const run = (context: unknown) => ({ trigger_context: context }) as unknown as WorkflowRun;
    expect(isShadowRun(run({ __variant__: 'shadow' }))).toBe(true);
    expect(isShadowRun(run({ __variant__: 'normal' }))).toBe(false);
    expect(isShadowRun(run(null))).toBe(false);
  });

  it('pretty-prints JSON envelopes, clips on code points and compares percentages', () => {
    const envelope = `---STEP_OUTPUT---\n${JSON.stringify({ data: { a: 1 }, status: 'OK' })}\n---END_STEP_OUTPUT---\n[SIGNAL: OK]`;
    expect(readableOutput(envelope)).toBe('{\n  "data": {\n    "a": 1\n  },\n  "status": "OK"\n}');
    expect(readableOutput('plain text\n[SIGNAL: OK]')).toBe('plain text');
    expect(clip('a😀b', 2)).toEqual({ text: 'a😀', clipped: true });
    expect(percent(100, 150)).toBe(' (+50 %)');
    expect(percent(100, 100)).toBe(' (=)');
    expect(percent(100, 40)).toBe(' (−60 %)');
    expect(percent(0, 40)).toBe('');
    expect(percent(null, 40)).toBe('');
  });
});

const exec = (shadowId: string | null, extra: Partial<ShadowExecution> = {}): ShadowExecution => ({
  shadow_id: shadowId ?? undefined,
  label: shadowId ?? '',
  status: 'success',
  identity: { agent: 'ClaudeCode', model: 'opus', prompt: 'qp-review@13', skills: ['rust'] },
  output: 'verdict',
  tokens_used: 100,
  duration_ms: 1000,
  cost_usd: 0.1,
  model: 'opus',
  html: 'none',
  ...extra,
});
const ollama = (shadowId: string, extra: Partial<ShadowExecution> = {}) => exec(shadowId, {
  label: 'Qwen (Ollama)',
  identity: { agent: 'Ollama', model: 'qwen3:30b', connection_id: 'conn-1', prompt: 'qp-review@13', skills: ['rust'] },
  model: 'qwen3:30b', output: 'qwen verdict', tokens_used: 400, duration_ms: 9000, cost_usd: undefined,
  ...extra,
});
const comparison = (main: ShadowExecution, ...shadows: ShadowExecution[]): StepShadowComparison => (
  { step_name: 'review', result_index: 1, main, shadows }
);

describe('what differs between the executions of a step (KT-1459)', () => {
  it('groups what every execution shares and names what differs', () => {
    const { identical, differing } = compareIdentities([exec(null), ollama('qwen'), exec('same')]);
    expect(differing).toEqual(['agent', 'model', 'connection']);
    expect(identical).toEqual([
      { field: 'prompt', value: 'qp-review@13' },
      { field: 'skills', value: 'rust' },
    ]);
  });

  it('says nothing differs between identical executions, skills compared whatever their order', () => {
    const a = exec(null, { identity: { agent: 'ClaudeCode', prompt: 'p', skills: ['b', 'a'] } });
    const b = exec('s', { identity: { agent: 'ClaudeCode', prompt: 'p', skills: ['a', 'b'] }, model: undefined });
    const a2 = { ...a, model: undefined };
    expect(compareIdentities([a2, b]).differing).toEqual([]);
    expect(sameIdentity(a2, b)).toBe(true);
  });

  it('counts the distinct outputs of the identical shadows out of N+1', () => {
    const step = comparison(exec(null, { output: 'A' }), exec('x', { output: 'A' }), exec('y', { output: 'B' }), ollama('qwen'));
    expect(determinism(step)).toEqual({ total: 3, distinct: 2, missing: 0 });
    // Timings and timestamps never make two outputs differ.
    const timed = comparison(exec(null, { output: 'done in 12 ms' }), exec('x', { output: 'done in 15 ms' }));
    expect(determinism(timed)).toEqual({ total: 2, distinct: 1, missing: 0 });
  });

  it('reports an identical shadow that gave no output, and none when no shadow is identical', () => {
    const failed = comparison(exec(null), exec('x', { status: 'failed', output: 'boom' }), exec('y', { status: 'running', output: '' }));
    expect(determinism(failed)).toEqual({ total: 1, distinct: 1, missing: 2 });
    expect(determinism(comparison(exec(null), ollama('qwen')))).toBeNull();
    expect(hasOutput(exec('x', { status: 'over_budget' }))).toBe(true);
    expect(hasOutput(exec('x', { status: 'timed_out', output: '' }))).toBe(false);
  });
});

describe('ShadowStatusButton', () => {
  it('states the shadows on the workflow page and opens their settings', () => {
    const fmt = (key: string, ...args: (string | number)[]) => (args.length ? `${key}(${args.join('|')})` : key);
    const projects = [{ id: 'p1', name: 'Alpha' }, { id: 'p2', name: 'Beta' }] as Project[];
    const onOpen = vi.fn();
    const { rerender } = render(<ShadowStatusButton steps={[step()]} projects={projects} onOpen={onOpen} t={fmt} />);
    const entry = screen.getByTestId('wf-shadow-entry');
    expect(entry).toHaveTextContent('wf.shadow.entryOff');
    expect(entry).toHaveAttribute('data-active', 'false');
    fireEvent.click(entry);
    expect(onOpen).toHaveBeenCalledTimes(1);

    const steps = [
      step({ name: 'a', shadows: [shadow('x'), shadow('y', { project_ids: ['p1'] })] }),
      step({ name: 'b', shadows: [shadow('z')] }),
      step({ name: 'c', shadows: [shadow('off', { enabled: false })] }),
    ];
    rerender(<ShadowStatusButton steps={steps} projects={projects} onOpen={onOpen} t={fmt} />);
    expect(entry).toHaveTextContent('wf.shadow.entryShadowsMany(3) · wf.shadow.entryStepsMany(2)');
    expect(entry).toHaveAttribute('title', 'wf.shadow.entryTitleOn(wf.shadow.entryAllProjects)');

    rerender(<ShadowStatusButton steps={[step({ shadows: [shadow('x', { project_ids: ['p2', 'gone'] })] })]} projects={projects} onOpen={onOpen} t={fmt} />);
    expect(entry).toHaveAttribute('title', 'wf.shadow.entryTitleOn(Beta, gone)');
    // One shadow on one step, merged: singular forms and the merge tag.
    rerender(<ShadowStatusButton steps={[step({ shadows: [shadow('x')], shadow_merge: { enabled: true, shadows: ['x'] } })]} projects={projects} onOpen={onOpen} t={fmt} />);
    expect(entry).toHaveTextContent('wf.shadow.entryShadowsOne(1) · wf.shadow.entryStepsOne(1) · wf.shadow.entryMerge');
  });
});

describe('StepShadowsEditor (KT-1459)', () => {
  const quickPrompts = [{ id: 'qp-alt', name: 'Alt prompt', skill_ids: [] }] as unknown as QuickPrompt[];
  const renderEditor = (target: WorkflowStep, onChange = vi.fn(), extra: Partial<Parameters<typeof StepShadowsEditor>[0]> = {}) => {
    render(
      <I18nProvider>
        <StepShadowsEditor step={target} onChange={onChange} projects={[]} skills={[]} quickPrompts={quickPrompts} t={t} {...extra} />
      </I18nProvider>,
    );
    return onChange;
  };

  it('adds an identical shadow with a fresh id, up to eight', () => {
    const onChange = renderEditor(step({ shadows: [shadow('shadow-2')] }));
    fireEvent.click(screen.getByTestId('wf-step-shadows-add'));
    expect(onChange).toHaveBeenLastCalledWith([shadow('shadow-2'), shadow('shadow-3')]);
  });

  it('refuses a ninth shadow', () => {
    const many = Array.from({ length: 8 }, (_, i) => shadow(`s${i}`));
    renderEditor(step({ shadows: many }));
    expect(screen.getByTestId('wf-step-shadows-add')).toBeDisabled();
  });

  it('starts collapsed on a step without shadows, then offers the first one', () => {
    const onChange = renderEditor(step());
    expect(screen.queryByTestId('wf-step-shadows-add')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /wf.shadow.title/ }));
    expect(screen.getByTestId('wf-step-shadows-none')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('wf-step-shadows-add'));
    expect(onChange).toHaveBeenLastCalledWith([shadow('shadow-1')]);
  });

  it('tells an identical shadow from one that changes something', () => {
    const { rerender } = render(
      <I18nProvider><StepShadowsEditor step={step({ shadows: [shadow('twin')] })} onChange={vi.fn()} projects={[]} skills={[]} quickPrompts={[]} t={t} /></I18nProvider>,
    );
    expect(screen.getByTestId('wf-shadow-twin-kind')).toHaveTextContent('wf.shadow.identicalHint');
    rerender(
      <I18nProvider><StepShadowsEditor step={step({ shadows: [shadow('twin', { agent: { agent: 'Codex' } })] })} onChange={vi.fn()} projects={[]} skills={[]} quickPrompts={[]} t={t} /></I18nProvider>,
    );
    expect(screen.getByTestId('wf-shadow-twin-kind')).toHaveTextContent('wf.shadow.changesHint');
  });

  it('edits the label, agent, model, prompt, sampling and budget of a shadow', () => {
    const onChange = renderEditor(step({ shadows: [shadow('qwen-local')] }));
    const set = (testId: string, value: string) => fireEvent.change(screen.getByTestId(testId), { target: { value } });
    set('wf-shadow-qwen-local-label', 'Qwen (Ollama)');
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen-local', { label: 'Qwen (Ollama)' })]);
    set('wf-shadow-qwen-local-agent', 'Ollama');
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen-local', { agent: { agent: 'Ollama' } })]);
    set('wf-shadow-qwen-local-prompt-kind', 'inline');
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen-local', { prompt: { type: 'inline', template: '' } })]);
    set('wf-shadow-qwen-local-prompt-kind', 'quick_prompt');
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen-local', { prompt: { type: 'quick_prompt', id: 'qp-alt' } })]);
    set('wf-shadow-qwen-local-sample', '3');
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen-local', { sample_every: 3 })]);
    set('wf-shadow-qwen-local-budget-tokens', '200000');
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen-local', { budget: { max_tokens: 200000 } })]);
    set('wf-shadow-qwen-local-budget-minutes', '120');
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen-local', { budget: { max_duration_secs: 7200 } })]);
    fireEvent.click(screen.getByTestId('wf-shadow-qwen-local-enabled'));
    expect(onChange).toHaveBeenLastCalledWith([shadow('qwen-local', { enabled: false })]);
  });

  it('keeps the model of the agent and names a connection for a custom one', () => {
    const onChange = renderEditor(step({ shadows: [shadow('alt', { agent: { agent: 'Custom' } })] }));
    fireEvent.change(screen.getByTestId('wf-shadow-alt-model'), { target: { value: 'gpt-x' } });
    expect(onChange).toHaveBeenLastCalledWith([shadow('alt', { agent: { agent: 'Custom', model: 'gpt-x' } })]);
    fireEvent.change(screen.getByTestId('wf-shadow-alt-connection'), { target: { value: 'litellm' } });
    expect(onChange).toHaveBeenLastCalledWith([shadow('alt', { agent: { agent: 'Custom', connection_id: 'litellm' } })]);
  });

  it('removes a shadow and flags a duplicate or an invalid id', () => {
    const onChange = renderEditor(step({ shadows: [shadow('a'), shadow('same'), shadow('same'), shadow('Bad Id')] }));
    const alerts = screen.getAllByRole('alert').map(alert => alert.textContent).join(' ');
    expect(alerts).toContain('wf.shadow.idTaken');
    expect(alerts).toContain('wf.shadow.idInvalid');
    fireEvent.click(screen.getByTestId('wf-shadow-a-remove'));
    expect(onChange).toHaveBeenLastCalledWith([shadow('same'), shadow('same'), shadow('Bad Id')]);
  });

  it('carries no shadow on a step that acts outside Kronn, or that is no Agent step', () => {
    const outside = renderEditor(step({ external_effect: true }), vi.fn(), { focusOnOpen: true, testId: 'outside' });
    expect(outside).not.toHaveBeenCalled();
    expect(screen.getByTestId('outside-ineligible')).toHaveTextContent('wf.shadow.outsideKronn');
    expect(screen.getByTestId('outside-add')).toBeDisabled();
  });

  it('says a Gate cannot carry shadows', () => {
    renderEditor(step({ step_type: { type: 'Gate' } } as Partial<WorkflowStep>), vi.fn(), { focusOnOpen: true });
    expect(screen.getByTestId('wf-step-shadows-ineligible')).toHaveTextContent('wf.shadow.onlyAgent');
  });

  it('pins the version of a custom skill the step uses, and offers none for the others', async () => {
    vi.mocked(skillsApi.history).mockResolvedValue(skillVersions(2, 1) as never);
    const onChange = renderEditor(step({ skill_ids: ['custom-review', 'rust'], shadows: [shadow('s')] }));
    const row = screen.getByTestId('wf-shadow-skill-custom-review');
    await waitFor(() => expect(within(row).getAllByRole('option')).toHaveLength(4));
    fireEvent.change(within(row).getByRole('combobox'), { target: { value: '1' } });
    expect(onChange).toHaveBeenLastCalledWith([shadow('s', { skills: { 'custom-review': 'custom-review@1' } })]);
    expect(screen.queryByTestId('wf-shadow-skill-rust')).toBeNull();
  });

  it('drops a skill in a shadow to measure what it brings', async () => {
    vi.mocked(skillsApi.history).mockResolvedValue(skillVersions(2) as never);
    const onChange = renderEditor(step({ skill_ids: ['custom-review'], shadows: [shadow('no-skill')] }));
    const row = screen.getByTestId('wf-shadow-skill-custom-review');
    await waitFor(() => expect(within(row).getAllByRole('option').length).toBeGreaterThan(2));
    fireEvent.change(within(row).getByRole('combobox'), { target: { value: 'drop' } });
    expect(onChange).toHaveBeenLastCalledWith([shadow('no-skill', { skills: { 'custom-review': '' } })]);
  });

  it('offers a version for the skills the step inherits from its Quick Prompt', async () => {
    vi.mocked(skillsApi.history).mockResolvedValue([]);
    const inherits = step({ quick_prompt_id: 'qp-review', shadows: [shadow('s')] });
    const prompts = [{ id: 'qp-review', name: 'Review', skill_ids: ['custom-review'] }] as unknown as QuickPrompt[];
    render(
      <I18nProvider><StepShadowsEditor step={inherits} onChange={vi.fn()} projects={[]} skills={[]} quickPrompts={prompts} t={t} /></I18nProvider>,
    );
    expect(await screen.findByTestId('wf-shadow-skill-custom-review')).toBeInTheDocument();
    expect(screen.getByTestId('wf-shadow-qp-qp-review')).toBeInTheDocument();
  });

  it('shows what the step loads and flags a shadow version that changes nothing', async () => {
    vi.mocked(skillsApi.history).mockResolvedValue(skillVersions(3, 2) as never);
    const pinned = step({ skill_ids: ['custom-review@2'], shadows: [shadow('s', { skills: { 'custom-review': 'custom-review@3' } })] });
    const projects = [{ id: 'p1', name: 'Site' }] as Project[];
    render(
      <I18nProvider>
        <StepShadowsEditor step={pinned} onChange={vi.fn()} projects={projects} skills={[]} quickPrompts={[]} t={(key, ...args) => [key, ...args].join(':')} />
      </I18nProvider>,
    );
    const row = screen.getByTestId('wf-shadow-skill-custom-review');
    await waitFor(() => expect(row.textContent).toContain('wf.shadow.optionLatest:3'));
    expect(row.textContent).toContain('wf.shadow.normalLoads:wf.shadow.pinnedVersion:2');
    // The step pins v2: v3 still differs, so no warning.
    expect(row.textContent).not.toContain('wf.shadow.sameVersionWarning');
    expect(screen.getByRole('button', { name: 'Site' })).toHaveAttribute('aria-pressed', 'false');
    // A shadow that tests the very version the step pins is told it changes nothing.
    cleanup();
    render(
      <I18nProvider>
        <StepShadowsEditor step={step({ skill_ids: ['custom-review@2'], shadows: [shadow('s', { skills: { 'custom-review': 'custom-review@2' } })] })} onChange={vi.fn()} projects={[]} skills={[]} quickPrompts={[]} t={t} />
      </I18nProvider>,
    );
    const same = screen.getByTestId('wf-shadow-skill-custom-review');
    await waitFor(() => expect(same.textContent).toContain('wf.shadow.sameVersionWarning'));
  });
});

describe('StepShadowCompare (KT-1459)', () => {
  const main = exec(null, { output: 'main verdict' });
  const same = exec('same', { output: 'main verdict', tokens_used: 100 });
  const again = exec('again', { output: 'another verdict', tokens_used: 110 });
  const qwen = ollama('qwen');
  const renderCompare = () => render(<I18nProvider><StepShadowCompare workflowId="wf" runId="r1" runStatus="Success" /></I18nProvider>);

  afterEach(() => { vi.useRealTimers(); });

  it('shows nothing for a run whose steps ran no shadow', async () => {
    const { container } = renderCompare();
    await waitFor(() => expect(workflowsApi.stepShadows).toHaveBeenCalledWith('wf', 'r1'));
    expect(container.querySelector('[data-testid="wf-step-shadow-compare"]')).toBeNull();
  });

  it('heads each execution with what differs and groups what is identical', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([comparison(main, same, qwen)]);
    renderCompare();
    const identical = await screen.findByTestId('wf-shadow-identical-review');
    // KT-1946 — a version reads « v13 »; the stored id stays in the tooltip.
    expect(identical.textContent).toContain('qp-review v13');
    expect(identical.querySelector('[title="qp-review@13"]')).not.toBeNull();
    expect(identical.textContent).toContain('rust');
    expect(identical.textContent).not.toContain('Ollama');

    const differs = (id: string) => screen.getByTestId(`wf-shadow-exec-review-${id}-differs`).textContent ?? '';
    expect(differs('main')).toContain('ClaudeCode');
    expect(differs('main')).toContain('opus');
    expect(differs('qwen')).toContain('Ollama');
    expect(differs('qwen')).toContain('qwen3:30b');
    expect(differs('qwen')).toContain('conn-1');
    expect(differs('qwen')).not.toContain('qp-review');
    // The identical shadow differs from the main one in nothing the header lists... but not from the Ollama one.
    expect(screen.getByTestId('wf-shadow-exec-review-same')).toBeInTheDocument();
  });

  it('gives each execution its tokens, cost and duration, and the change against the main one', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([comparison(main, qwen)]);
    renderCompare();
    const metrics = (id: string) => screen.findByTestId(`wf-shadow-exec-review-${id}-metrics`);
    const mainMetrics = await metrics('main');
    expect(mainMetrics.textContent).toContain('100');
    expect(mainMetrics.textContent).toContain('$0.10');
    expect(mainMetrics.textContent).toContain('1s');
    const qwenMetrics = await metrics('qwen');
    expect(qwenMetrics.textContent).toContain('400');
    expect(qwenMetrics.textContent).toContain('+300 %');
    expect(qwenMetrics.textContent).toContain('—');
    expect(qwenMetrics.textContent).toContain('9s');
  });

  it('summarises how many distinct outputs the identical shadows gave, out of N+1', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([comparison(main, same, again, qwen)]);
    renderCompare();
    const summary = await screen.findByTestId('wf-shadow-determinism-review');
    expect(summary).toHaveAttribute('data-distinct', '2');
    expect(summary.textContent).toContain('2 sortie(s) distincte(s) sur 3');
  });

  it('diffs each shadow against the main output', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([comparison(main, same, qwen)]);
    renderCompare();
    const different = await screen.findByTestId('wf-shadow-vs-main-review-qwen');
    expect(different).toHaveAttribute('data-verdict', 'different');
    fireEvent.click(different.querySelector('summary')!);
    const diff = await screen.findByTestId('wf-shadow-diff-review-qwen');
    expect(diff.textContent).toContain('main verdict');
    expect(diff.textContent).toContain('qwen verdict');
    const alike = screen.getByTestId('wf-shadow-vs-main-review-same');
    expect(alike).toHaveAttribute('data-verdict', 'same');
    expect(screen.queryByTestId('wf-shadow-diff-review-same')).toBeNull();
  });

  it('compares any two executions side by side', async () => {
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([comparison(main, again, qwen)]);
    renderCompare();
    const left = await screen.findByTestId('wf-shadow-pick-left-review');
    fireEvent.change(left, { target: { value: 'qwen' } });
    fireEvent.change(screen.getByTestId('wf-shadow-pick-right-review'), { target: { value: 'again' } });
    const diff = await screen.findByTestId('wf-shadow-pair-diff-review');
    expect(diff.textContent).toContain('qwen verdict');
    expect(diff.textContent).toContain('another verdict');
  });

  it('reports a shadow that was stopped and keeps the rest of the comparison', async () => {
    const stopped = ollama('slow', { status: 'timed_out', output: '', stopped: 'Stopped by the shadow\'s time budget: 3600 s elapsed', tokens_used: undefined, duration_ms: 3_600_000 });
    vi.mocked(workflowsApi.stepShadows).mockResolvedValue([comparison(main, stopped)]);
    renderCompare();
    const card = await screen.findByTestId('wf-shadow-exec-review-slow');
    expect(card).toHaveAttribute('data-status', 'timed_out');
    expect(card.textContent).toContain('time budget');
    expect(screen.getByTestId('wf-shadow-vs-main-review-slow')).toHaveAttribute('data-verdict', 'none');
  });

  it('keeps refreshing while a shadow still runs', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const running = ollama('local', { status: 'running', output: '', tokens_used: undefined, duration_ms: undefined });
    vi.mocked(workflowsApi.stepShadows)
      .mockResolvedValueOnce([comparison(main, running)])
      .mockResolvedValue([comparison(main, ollama('local'))]);
    renderCompare();
    await screen.findByTestId('wf-shadow-exec-review-local');
    expect(screen.getByTestId('wf-shadow-exec-review-local')).toHaveAttribute('data-status', 'running');
    await act(async () => { await vi.advanceTimersByTimeAsync(3100); });
    await waitFor(() => expect(screen.getByTestId('wf-shadow-exec-review-local')).toHaveAttribute('data-status', 'success'));
    expect(vi.mocked(workflowsApi.stepShadows).mock.calls.length).toBeGreaterThanOrEqual(2);
  });
});

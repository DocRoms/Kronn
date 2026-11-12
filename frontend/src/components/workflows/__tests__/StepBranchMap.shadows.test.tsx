// KT-1696 — a doubled step on the branch map: its ⑂ / ⊕ tags, one line per
// shadow (struck through when paused), the merged-result line, the footer.
// KT-1707 — the principal line comes first, then `⑂ Agent · model` lines.
import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import { StepBranchMap } from '../StepBranchMap';
import type { ShadowMerge, StepShadow, WorkflowStep } from '../../../types/generated';

const t = (key: string, ...args: (string | number)[]) => (args.length ? `${key}:${args.join(',')}` : key);

const step = (name: string, extra: Partial<WorkflowStep> = {}): WorkflowStep => ({
  name, step_type: { type: 'Agent' }, agent: 'Ollama', agent_settings: { model: 'qwen3.8:27b-mlx' }, prompt_template: '', on_result: [], ...extra,
}) as unknown as WorkflowStep;
const shadow = (id: string, extra: Partial<StepShadow> = {}): StepShadow => ({ id, label: '', enabled: true, ...extra });
const merge = (extra: Partial<ShadowMerge> = {}): ShadowMerge => ({ enabled: true, ...extra });

afterEach(cleanup);

describe('StepBranchMap — step shadows', () => {
  const steps = [
    step('collect'),
    step('advise', {
      shadows: [shadow('argon', { label: 'Argon', agent: { agent: 'LiteLlm', model: 'gemini-4-argon' } }), shadow('twin', { enabled: false })],
      shadow_merge: merge({ shadows: ['argon'] }),
    }),
    step('publish'),
  ];

  it('tags the step with ⑂ N and ⊕, and draws one line per shadow named by agent and model', () => {
    render(<StepBranchMap steps={steps} t={t} />);
    expect(screen.getByTestId('wf-bm-shadow-tag-1')).toHaveTextContent('⑂ 2');
    expect(screen.getByTestId('wf-bm-merge-tag-1')).toHaveTextContent('⊕');
    expect(screen.getByTestId('wf-bm-shadow-1-argon')).toHaveTextContent('⑂ LiteLLM · gemini-4-argon');
    // A shadow on the principal's own agent shows that agent and model.
    expect(screen.getByTestId('wf-bm-shadow-1-twin')).toHaveTextContent('⑂ Ollama · qwen3.8:27b-mlx');
  });

  it('lists the principal line before the shadow lines, and only for a doubled step', () => {
    render(<StepBranchMap steps={steps} t={t} />);
    const principal = screen.getByTestId('wf-bm-principal-1');
    expect(principal).toHaveTextContent('wf.branchMap.principal:Ollama · qwen3.8:27b-mlx');
    const argon = screen.getByTestId('wf-bm-shadow-1-argon');
    const twin = screen.getByTestId('wf-bm-shadow-1-twin');
    const y = (node: HTMLElement) => Number(node.getAttribute('y'));
    expect(y(principal)).toBeLessThan(y(argon));
    expect(y(argon)).toBeLessThan(y(twin));
    expect(y(twin)).toBeLessThan(y(screen.getByTestId('wf-bm-merged-1')));
    expect(principal.compareDocumentPosition(argon) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.queryByTestId('wf-bm-principal-0')).toBeNull();
    expect(screen.queryByTestId('wf-bm-principal-2')).toBeNull();
  });

  it('keeps the free label as a tooltip of the shadow line, not as its text', () => {
    const named = [steps[0], step('advise', {
      shadows: [shadow('argon', { label: 'Argon (LiteLLM)', agent: { agent: 'LiteLlm', model: 'gemini-4-argon' } }), shadow('qwen', { label: 'Qwen identique' })],
      shadow_merge: merge({ shadows: ['argon'] }),
    }), steps[2]];
    render(<StepBranchMap steps={named} t={t} />);
    const line = screen.getByTestId('wf-bm-shadow-1-argon');
    expect(line).toHaveTextContent('⑂ LiteLLM · gemini-4-argon');
    expect(line.querySelector('title')).toHaveTextContent('Argon (LiteLLM)');
    expect(screen.getByTestId('wf-bm-shadow-1-qwen').querySelector('title')).toHaveTextContent('Qwen identique');
  });

  it('clips a long agent · model only past the map column', () => {
    const model = 'an-extremely-long-model-identifier-that-cannot-fit-the-column';
    render(<StepBranchMap steps={[step('advise', { shadows: [shadow('long', { agent: { agent: 'LiteLlm', model } })] })]} t={t} />);
    const line = screen.getByTestId('wf-bm-shadow-0-long');
    const drawn = Array.from(line.childNodes).filter(node => node.nodeType === Node.TEXT_NODE).map(node => node.textContent).join('');
    expect(drawn).toContain('…');
    expect(drawn).not.toContain(model);
  });

  it('strikes a paused shadow through, not an active one', () => {
    render(<StepBranchMap steps={steps} t={t} />);
    expect(screen.getByTestId('wf-bm-shadow-1-twin')).toHaveAttribute('data-paused', 'true');
    expect(screen.getByTestId('wf-bm-shadow-1-argon')).toHaveAttribute('data-paused', 'false');
  });

  it('ends the doubled step with the merged result and the step it goes to', () => {
    render(<StepBranchMap steps={steps} t={t} />);
    expect(screen.getByTestId('wf-bm-merged-1')).toHaveTextContent('wf.branchMap.mergedTo:publish');
  });

  it('has no merged-result line, nor ⊕, when nothing is merged', () => {
    const unmerged = [steps[0], { ...steps[1], shadow_merge: merge({ enabled: false, shadows: [] }) }, steps[2]];
    render(<StepBranchMap steps={unmerged} t={t} />);
    expect(screen.getByTestId('wf-bm-shadow-tag-1')).toBeInTheDocument();
    expect(screen.queryByTestId('wf-bm-merge-tag-1')).toBeNull();
    expect(screen.queryByTestId('wf-bm-merged-1')).toBeNull();
  });

  it('does not name a next step for the last one', () => {
    render(<StepBranchMap steps={[steps[0], steps[1]]} t={t} />);
    expect(screen.getByTestId('wf-bm-merged-1')).toHaveTextContent('wf.branchMap.mergedLast');
  });

  it('reads « linear · N steps doubled » instead of « no branches », with plurals', () => {
    const { unmount } = render(<StepBranchMap steps={steps} t={t} />);
    expect(screen.getByTestId('wf-bm-legend-linear')).toHaveTextContent('wf.branchMap.linearDoubledOne:1');
    unmount();
    render(<StepBranchMap steps={[steps[0], steps[1], { ...steps[1], name: 'again' }]} t={t} />);
    expect(screen.getByTestId('wf-bm-legend-linear')).toHaveTextContent('wf.branchMap.linearDoubledMany:2');
  });

  it('keeps « no branches » for a workflow without shadows', () => {
    render(<StepBranchMap steps={[steps[0], steps[2]]} t={t} />);
    expect(screen.getByTestId('wf-bm-legend-linear')).toHaveTextContent('wf.branchMap.linear');
    expect(screen.queryByTestId('wf-bm-shadow-tag-0')).toBeNull();
  });

  it('makes room for the lines: the map grows and the next node moves down', () => {
    const plain = render(<StepBranchMap steps={[steps[0], steps[2]]} t={t} />);
    const plainHeight = Number(plain.container.querySelector('svg')?.getAttribute('height'));
    plain.unmount();
    render(<StepBranchMap steps={steps} t={t} />);
    const doubledHeight = Number(document.querySelector('svg.wf-branch-map')?.getAttribute('height'));
    expect(doubledHeight).toBeGreaterThan(plainHeight + 24);
    const nodes = screen.getAllByTestId(/^wf-bm-node-\d$/).map(node => Number(node.querySelector('circle')?.getAttribute('cy')));
    expect(nodes[1]).toBeGreaterThan(nodes[0]);
    // Four lines (principal, two shadows, merged) push `publish` well past a plain row.
    expect(nodes[2] - nodes[1]).toBeGreaterThan(24 + 4 * 10);
  });
});

// KT-1707 — the one helper behind the three places that name an agent's model: the
// shadow rows (and the « merged by » menu), and the branch map. A step that sets no
// model is named by the model its agent is configured to run, as the step's own picker
// does; « default model » stays only for an agent nothing configures.
import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen, within } from '@testing-library/react';
import { buildApiMock } from '../../../test/apiMock';
import type { ModelTiersConfig, ShadowMerge, StepShadow, WorkflowStep } from '../../../types/generated';

vi.mock('../../../lib/api', () => buildApiMock());

import { StepShadowRows } from '../StepShadowRows';
import { StepBranchMap } from '../StepBranchMap';
import { agentModelLabel, resolveAgentModel, shadowDifferences, stepLoadLabel, stepMainAgent } from '../shadowRowModel';

const t = (key: string, ...args: (string | number)[]) => (key === 'wf.shadow.rows.defaultModel' ? 'DEFAULT-MODEL' : args.length ? `${key}(${args.join('|')})` : key);

const familyTiers = {
  ollama: { economy: null, default: 'qwen3.8:27b-mlx', reasoning: null },
  open_code: { economy: null, default: 'opencode-go/kimi-k3.1-thinking', reasoning: 'opencode-go/kimi-k3.1-max' },
} as unknown as ModelTiersConfig;
const choices = [
  { agent: 'Ollama' as const },
  { agent: 'LiteLlm' as const, connectionId: 'lite-1', label: 'LiteLLM', modelTiers: { economy: null, default: 'gemini-4-argon', reasoning: null } },
  { agent: 'OpenCode' as const },
  { agent: 'ClaudeCode' as const },
];

const shadow = (id: string, extra: Partial<StepShadow> = {}): StepShadow => ({ id, label: '', enabled: true, ...extra });
const merge = (extra: Partial<ShadowMerge> = {}): ShadowMerge => ({ enabled: true, ...extra });
// No model on the step: the agent's configured one is what runs.
const step = (extra: Partial<WorkflowStep> = {}): WorkflowStep => ({
  name: 'advise', agent: 'Ollama', step_type: { type: 'Agent' }, agent_settings: {}, skill_ids: [], on_result: [], prompt_template: '', ...extra,
}) as unknown as WorkflowStep;
const doubled = step({
  shadows: [
    shadow('twin', { label: 'Qwen identique' }),
    shadow('argon', { label: 'Argon', agent: { agent: 'LiteLlm', connection_id: 'lite-1' } }),
    shadow('kimi', { label: 'Kimi', agent: { agent: 'OpenCode' } }),
    shadow('bare', { label: 'Bare', agent: { agent: 'ClaudeCode' } }),
  ],
  shadow_merge: merge({ shadows: ['twin', 'argon'] }),
});

afterEach(cleanup);

describe('resolveAgentModel / agentModelLabel', () => {
  it('uses the explicit model first', () => {
    expect(resolveAgentModel({ agent: 'Ollama', model: 'llama4:8b' }, choices, familyTiers)).toBe('llama4:8b');
  });

  it("resolves the agent family's model for the step's tier", () => {
    expect(resolveAgentModel(stepMainAgent(step()), choices, familyTiers)).toBe('qwen3.8:27b-mlx');
    expect(resolveAgentModel(stepMainAgent(step({ agent: 'OpenCode', agent_settings: { tier: 'reasoning' } } as never)), choices, familyTiers))
      .toBe('opencode-go/kimi-k3.1-max');
  });

  it("resolves a connection's own tiers, and a shadow's agent on the default tier", () => {
    expect(resolveAgentModel({ agent: 'LiteLlm', connectionId: 'lite-1' }, choices, familyTiers)).toBe('gemini-4-argon');
  });

  it('is empty when nothing is configured, and the label then says « default model »', () => {
    expect(resolveAgentModel({ agent: 'ClaudeCode' }, choices, familyTiers)).toBe('');
    expect(agentModelLabel({ agent: 'ClaudeCode' }, stepMainAgent(step()), choices, t, familyTiers)).toBe('Claude Code · DEFAULT-MODEL');
  });

  it('names a shadow without an agent after the principal, resolved', () => {
    expect(agentModelLabel(undefined, stepMainAgent(step()), choices, t, familyTiers)).toBe('Ollama · qwen3.8:27b-mlx');
    expect(agentModelLabel({ agent: 'LiteLlm', connection_id: 'lite-1' }, stepMainAgent(step()), choices, t, familyTiers)).toBe('LiteLLM · gemini-4-argon');
  });
});

describe('one resolved model on the three surfaces', () => {
  it('shadow rows: the shadow without an agent, the shadow with a default model, and the principal', () => {
    render(<StepShadowRows step={doubled} nextStepName="publish" choices={choices} modelTiers={familyTiers} t={t} />);
    expect(within(screen.getByTestId('wf-shadow-rows-twin')).getByTestId('wf-shadow-rows-twin-agent')).toHaveTextContent('Ollama · qwen3.8:27b-mlx');
    expect(screen.getByTestId('wf-shadow-rows-argon-agent')).toHaveTextContent('LiteLLM · gemini-4-argon');
    expect(screen.getByTestId('wf-shadow-rows-kimi-agent')).toHaveTextContent('OpenCode · opencode-go/kimi-k3.1-thinking');
    expect(screen.getByTestId('wf-shadow-rows-principal-agent')).toHaveTextContent('Ollama · qwen3.8:27b-mlx');
    // Nothing configures Claude Code's model: « default model » is all that can be said.
    expect(screen.getByTestId('wf-shadow-rows-bare-agent')).toHaveTextContent('Claude Code · wf.shadow.rows.defaultModel'); // the shared picker names it in the app's language (KT-1885)
    for (const id of ['principal', 'twin', 'argon', 'kimi']) expect(screen.getByTestId(`wf-shadow-rows-${id}-agent`)).not.toHaveTextContent('wf.shadow.rows.defaultModel');
  });

  it('shadow rows: the summary and the « merged by » menu name the resolved model', () => {
    render(<StepShadowRows step={doubled} nextStepName="publish" choices={choices} modelTiers={familyTiers} t={t} />);
    expect(screen.getByTestId('wf-shadow-rows-merged')).toHaveTextContent(
      'wf.shadow.rows.shadowNamed(Ollama · qwen3.8:27b-mlx) + wf.shadow.rows.shadowNamed(LiteLLM · gemini-4-argon)',
    );
    expect(screen.getByTestId('wf-shadow-rows-condense')).toHaveTextContent('Ollama · qwen3.8:27b-mlx');
    expect(screen.getByTestId('wf-shadow-rows-condense')).not.toHaveTextContent('DEFAULT-MODEL');
  });

  it('shadow rows: the full agent · model is the tooltip of the picker', () => {
    render(<StepShadowRows step={doubled} nextStepName="publish" choices={choices} modelTiers={familyTiers} t={t} />);
    expect(screen.getByTestId('wf-shadow-rows-kimi-agent')).toHaveAttribute('title', 'OpenCode · opencode-go/kimi-k3.1-thinking');
  });

  it('branch map: the principal line and the shadow lines', () => {
    render(<StepBranchMap steps={[doubled]} choices={choices} modelTiers={familyTiers} t={t} />);
    expect(screen.getByTestId('wf-bm-principal-0')).toHaveTextContent('wf.branchMap.principal(Ollama · qwen3.8:27b-mlx)');
    expect(screen.getByTestId('wf-bm-shadow-0-twin')).toHaveTextContent('⑂ Ollama · qwen3.8:27b-mlx');
    expect(screen.getByTestId('wf-bm-shadow-0-argon')).toHaveTextContent('⑂ LiteLLM · gemini-4-argon');
    expect(screen.getByTestId('wf-bm-shadow-0-bare')).toHaveTextContent('⑂ Claude Code · DEFAULT-MODEL');
    for (const id of ['twin', 'argon', 'kimi']) expect(screen.getByTestId(`wf-bm-shadow-0-${id}`)).not.toHaveTextContent('DEFAULT-MODEL');
    expect(screen.getByTestId('wf-bm-principal-0')).not.toHaveTextContent('DEFAULT-MODEL');
  });

  it('branch map: a line clipped to the column keeps its full label as a tooltip', () => {
    const long = step({ shadows: [shadow('kimi', { label: 'Kimi', agent: { agent: 'OpenCode', model: 'opencode-go/kimi-k3.1-thinking-with-a-very-long-suffix' } })] });
    render(<StepBranchMap steps={[long]} choices={choices} modelTiers={familyTiers} t={t} />);
    const line = screen.getByTestId('wf-bm-shadow-0-kimi');
    expect(line.textContent).toContain('…');
    expect(line.querySelector('title')?.textContent).toContain('OpenCode · opencode-go/kimi-k3.1-thinking-with-a-very-long-suffix');
    expect(line.querySelector('title')?.textContent).toContain('Kimi');
  });
});

// KT-1946 — two shadows on the same agent read the same on a row while one of them
// runs another rubric: each row says the prompt and skills it runs with.
describe('what a row loads', () => {
  const names = {
    quickPrompts: [
      { id: 'qp-review', name: 'PR review', skill_ids: ['custom-rubric@8', 'custom-checklist@3', 'git-conventions'] },
      { id: 'qp-strict', name: 'PR review strict', skill_ids: ['custom-rubric', 'security'] },
    ],
    skills: [{ id: 'custom-rubric', name: 'pr-review-generic' }, { id: 'custom-checklist', name: 'security-checklist' }, { id: 'custom-other', name: 'other-rubric' }],
  };
  const reviewed = step({ quick_prompt_id: 'qp-review', quick_prompt_version: 8 });
  const diff = (extra: Partial<StepShadow>, of: WorkflowStep = reviewed) => shadowDifferences(of, shadow('s', extra), names, t);

  it('names the principal\'s prompt, its version and the skills its prompt brings', () => {
    expect(stepLoadLabel(reviewed, names, t)).toBe('wf.shadow.rows.load.promptVersion(PR review|8) · wf.shadow.rows.load.skills(3)');
    expect(stepLoadLabel(step({ skill_ids: ['custom-rubric@2'] }), names, t)).toBe('wf.shadow.rows.load.skill(1)');
    expect(stepLoadLabel(step(), names, t)).toBe('');
  });

  it('says nothing differs for a shadow that only changes agent', () => {
    expect(diff({ agent: { agent: 'OpenCode' } })).toEqual([]);
    // Pinning what the step already runs is not a difference.
    expect(diff({ quick_prompt_versions: { 'qp-review': 8 }, skills: { 'custom-rubric': 'custom-rubric@8' } })).toEqual([]);
  });

  it('names a skill version, a dropped skill and a replaced one', () => {
    expect(diff({ skills: { 'custom-rubric': 'custom-rubric@9' } })).toEqual(['wf.shadow.rows.diff.skillVersion(pr-review-generic|v8|v9)']);
    expect(diff({ skills: { 'custom-checklist': '' } })).toEqual(['wf.shadow.rows.diff.skillDropped(security-checklist)']);
    expect(diff({ skills: { 'custom-rubric': 'custom-other@2' } })).toEqual(['wf.shadow.rows.diff.skillReplaced(pr-review-generic|other-rubric)']);
    // A skill the step does not load changes nothing.
    expect(diff({ skills: { 'custom-absent': 'custom-absent@4' } })).toEqual([]);
  });

  it('names another version of the same prompt, another prompt with its own skills, and a written prompt', () => {
    expect(diff({ quick_prompt_versions: { 'qp-review': 7 } })).toEqual(['wf.shadow.rows.diff.promptChange(PR review|v8|v7)']);
    expect(diff({ prompt: { type: 'quick_prompt', id: 'qp-strict', version: 2 } }))
      .toEqual(['wf.shadow.rows.load.promptVersion(PR review strict|2)', 'wf.shadow.rows.diff.ownSkills(2)']);
    // Another prompt replaces the step's own skills too (`apply_prompt`), and a swap applies to what it brings.
    expect(diff({ prompt: { type: 'quick_prompt', id: 'qp-strict' }, skills: { 'custom-rubric': 'custom-rubric@4', 'custom-checklist': '' } },
      step({ quick_prompt_id: 'qp-review', skill_ids: ['custom-checklist'] })))
      .toEqual(['wf.shadow.rows.load.prompt(PR review strict)', 'wf.shadow.rows.diff.ownSkills(2)', 'wf.shadow.rows.diff.skillVersion(pr-review-generic|wf.shadow.rows.diff.current|v4)']);
    // A written prompt leaves the skills the Quick Prompt brought, and keeps the step's own.
    expect(diff({ prompt: { type: 'inline', template: 'Review it.' } }))
      .toEqual(['wf.shadow.rows.diff.promptInline', 'wf.shadow.rows.diff.promptSkillsGone']);
    expect(diff({ prompt: { type: 'inline', template: 'x' } }, step({ quick_prompt_id: 'qp-review', skill_ids: ['custom-rubric'] })))
      .toEqual(['wf.shadow.rows.diff.promptInline']);
  });

  it('counts the skills beyond the first when more than two differ', () => {
    const three = step({ skill_ids: ['custom-rubric@8', 'custom-checklist@3', 'custom-other@1'] });
    expect(diff({ skills: { 'custom-rubric': 'custom-rubric@9', 'custom-checklist': '', 'custom-other': 'custom-other@2' } }, three))
      .toEqual(['wf.shadow.rows.diff.skillVersion(pr-review-generic|v8|v9)', 'wf.shadow.rows.diff.more(2)']);
  });

  it('shows the line on the principal and on each shadow row, and none without names', () => {
    const shadowed = step({ quick_prompt_id: 'qp-review', quick_prompt_version: 8, shadows: [shadow('twin'), shadow('next', { skills: { 'custom-rubric': 'custom-rubric@9' } })] });
    const { unmount } = render(<StepShadowRows step={shadowed} choices={choices} modelTiers={familyTiers} names={names} t={t} />);
    expect(screen.getByTestId('wf-shadow-rows-principal-load')).toHaveTextContent('PR review');
    expect(screen.getByTestId('wf-shadow-rows-twin-load')).toHaveAttribute('data-same', 'true');
    expect(screen.getByTestId('wf-shadow-rows-twin-load')).toHaveTextContent('wf.shadow.rows.diff.same');
    expect(screen.getByTestId('wf-shadow-rows-next-load')).toHaveTextContent('wf.shadow.rows.diff.skillVersion(pr-review-generic|v8|v9)');
    unmount();
    render(<StepShadowRows step={shadowed} choices={choices} modelTiers={familyTiers} t={t} />);
    expect(screen.queryByTestId('wf-shadow-rows-twin-load')).toBeNull();
  });
});

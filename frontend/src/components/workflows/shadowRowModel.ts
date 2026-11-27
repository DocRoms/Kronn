/**
 * KT-1707 — what the rows of a shadowed step say: whether the step reads as a block
 * (Principal row + shadows), and the `Agent · model` a row names.
 */
import { AGENT_LABELS } from '../../lib/constants';
import type { AgentSwitchTarget, MainAgent } from '../AgentSwitchPicker';
import { stepCanCarryShadows } from './shadowEditorModel';
import { resolveCatalogTier } from '../../lib/modelCatalogSelection';
import { skillBaseId, skillPinnedVersion } from '../../lib/automationSkills';
import type { ModelTiersConfig, QuickPrompt, ShadowAgent, Skill, StepShadow, WorkflowStep } from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** Whether the step reads as a block (Principal row + shadows) rather than as its single agent line. */
export const hasShadowBlock = (step: WorkflowStep) => stepCanCarryShadows(step) && (step.shadows?.length ?? 0) > 0;

/** The agent a step runs on: what its picker shows, and what a shadow without an agent of its own runs on. */
export const stepMainAgent = (step: WorkflowStep): MainAgent => ({
  agent: step.agent,
  connectionId: step.agent_settings?.connection_id,
  model: step.agent_settings?.model,
  tier: step.agent_settings?.tier ?? 'default',
});

/**
 * The model an agent runs, resolved the way the step's own picker names it: the explicit
 * model, else the one configured for its tier (the connection's, or the agent family's).
 * Empty when nothing is configured.
 */
export function resolveAgentModel(agent: MainAgent, choices: readonly AgentSwitchTarget[], familyTiers?: ModelTiersConfig | null): string {
  const connectionId = agent.connectionId ?? null;
  const known = choices.find(target => target.agent === agent.agent && (target.connectionId ?? null) === connectionId);
  const target = known ?? { agent: agent.agent, connectionId };
  return resolveCatalogTier(null, target, agent.tier ?? 'default', familyTiers, agent.model).model || '';
}

/**
 * What a row, or a branch-map line, shows for an agent: `Agent · model`. A shadow without
 * an agent of its own runs on the principal, so it shows the principal's agent and model
 * resolved. « default model » only stands for a model nothing configures.
 */
export function agentModelLabel(
  value: ShadowAgent | undefined,
  main: MainAgent,
  choices: readonly AgentSwitchTarget[],
  t: Translate,
  familyTiers?: ModelTiersConfig | null,
): string {
  const resolved: MainAgent = value ? { agent: value.agent, connectionId: value.connection_id, model: value.model } : main;
  const connectionId = resolved.connectionId ?? null;
  const known = choices.find(target => target.agent === resolved.agent && (target.connectionId ?? null) === connectionId);
  const agent = known?.label ?? AGENT_LABELS[resolved.agent] ?? resolved.agent;
  return `${agent} · ${resolveAgentModel(resolved, choices, familyTiers) || t('wf.shadow.rows.defaultModel')}`;
}

/** The names a row needs to say what it loads; an id stands for a name that is not loaded yet. */
export interface LoadNames {
  quickPrompts?: readonly Pick<QuickPrompt, 'id' | 'name' | 'skill_ids'>[];
  skills?: readonly Pick<Skill, 'id' | 'name'>[];
}

const promptName = (id: string, names: LoadNames) => names.quickPrompts?.find(qp => qp.id === id)?.name ?? id;
const skillName = (id: string, names: LoadNames) => {
  const base = skillBaseId(id);
  return names.skills?.find(skill => skill.id === base)?.name ?? base.replace(/^custom-/, '');
};
/** What the step loads: its own skills, else its Quick Prompt's. */
const stepSkillIds = (step: WorkflowStep, names: LoadNames): readonly string[] =>
  step.skill_ids?.length ? step.skill_ids : names.quickPrompts?.find(qp => qp.id === step.quick_prompt_id)?.skill_ids ?? [];

/**
 * What a shadow loads, as the engine resolves it (`apply_prompt`): another Quick Prompt
 * replaces the step's own skills by that prompt's, and a written prompt leaves only the
 * skills the step names itself.
 */
export function shadowSkillIds(step: WorkflowStep, shadow: StepShadow, names: LoadNames): readonly string[] {
  const prompt = shadow.prompt;
  if (prompt?.type === 'quick_prompt' && prompt.id !== step.quick_prompt_id) {
    return names.quickPrompts?.find(qp => qp.id === prompt.id)?.skill_ids ?? [];
  }
  if (prompt?.type === 'inline') return step.skill_ids ?? [];
  return stepSkillIds(step, names);
}

/** KT-1946 — the prompt and skills the principal runs with, e.g. « PR review v8 · 3 skills ». Empty when it names neither. */
export function stepLoadLabel(step: WorkflowStep, names: LoadNames, t: Translate): string {
  const parts: string[] = [];
  if (step.quick_prompt_id) {
    const name = promptName(step.quick_prompt_id, names);
    parts.push(step.quick_prompt_version ? t('wf.shadow.rows.load.promptVersion', name, step.quick_prompt_version) : t('wf.shadow.rows.load.prompt', name));
  }
  const count = new Set(stepSkillIds(step, names).map(skillBaseId)).size;
  if (count > 0) parts.push(t(count === 1 ? 'wf.shadow.rows.load.skill' : 'wf.shadow.rows.load.skills', count));
  return parts.join(' · ');
}

/**
 * KT-1946 — what a shadow loads that the principal does not, one short phrase per
 * difference. Empty when the shadow runs the same prompt and skills.
 */
export function shadowDifferences(step: WorkflowStep, shadow: StepShadow, names: LoadNames, t: Translate): string[] {
  const out: string[] = [];
  const version = (n: number | null | undefined) => (n ? `v${n}` : t('wf.shadow.rows.diff.current'));
  const prompt = shadow.prompt;
  let otherPrompt = false;
  if (prompt?.type === 'inline') {
    out.push(t('wf.shadow.rows.diff.promptInline'));
    otherPrompt = true;
  } else if (prompt?.type === 'quick_prompt' && prompt.id && prompt.id !== step.quick_prompt_id) {
    const name = promptName(prompt.id, names);
    out.push(prompt.version ? t('wf.shadow.rows.load.promptVersion', name, prompt.version) : t('wf.shadow.rows.load.prompt', name));
    otherPrompt = true;
    // Another prompt brings its own skills, whatever the step named.
    out.push(t('wf.shadow.rows.diff.ownSkills', new Set(shadowSkillIds(step, shadow, names).map(skillBaseId)).size));
  }
  // A written prompt leaves the skills the step's Quick Prompt brought.
  if (prompt?.type === 'inline' && !step.skill_ids?.length && stepSkillIds(step, names).length > 0) {
    out.push(t('wf.shadow.rows.diff.promptSkillsGone'));
  }
  if (!otherPrompt && step.quick_prompt_id) {
    const pinned = (prompt?.type === 'quick_prompt' ? prompt.version : undefined) ?? shadow.quick_prompt_versions?.[step.quick_prompt_id];
    if (pinned && pinned !== (step.quick_prompt_version ?? null)) {
      out.push(t('wf.shadow.rows.diff.promptChange', promptName(step.quick_prompt_id, names), version(step.quick_prompt_version), version(pinned)));
    }
  }
  const loaded = new Map(shadowSkillIds(step, shadow, names).map(id => [skillBaseId(id), skillPinnedVersion(id)] as const));
  const skills: string[] = [];
  for (const [base, replacement] of Object.entries(shadow.skills ?? {})) {
    if (!loaded.has(base)) continue;
    const name = skillName(base, names);
    if (replacement === '') skills.push(t('wf.shadow.rows.diff.skillDropped', name));
    else if (skillBaseId(replacement) !== base) skills.push(t('wf.shadow.rows.diff.skillReplaced', name, skillName(replacement, names)));
    else if (skillPinnedVersion(replacement) !== loaded.get(base)) {
      skills.push(t('wf.shadow.rows.diff.skillVersion', name, version(loaded.get(base)), version(skillPinnedVersion(replacement))));
    }
  }
  // Beyond two skills the row names the first and counts the rest.
  if (skills.length > 2) skills.splice(1, skills.length, t('wf.shadow.rows.diff.more', skills.length - 1));
  return [...out, ...skills];
}

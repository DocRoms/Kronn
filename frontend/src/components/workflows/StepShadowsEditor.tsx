/**
 * KT-1449, KT-1459 — the shadows of one step: each re-runs only this step
 * beside the main execution, on exactly the same input. A shadow may change
 * nothing (to measure how deterministic the step is), or its agent, model or
 * connection, its prompt, its skills. A step that acts outside Kronn, and a
 * step that is no Agent step, carries none.
 */
import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, ChevronDown, ChevronRight, FlaskConical, Info, Plus, Trash2 } from 'lucide-react';
import { skills as skillsApi, quickPrompts as quickPromptsApi } from '../../lib/api';
import { skillBaseId, skillPinnedVersion } from '../../lib/automationSkills';
import { AgentSwitchPicker, type AgentSwitchTarget, type MainAgent } from '../AgentSwitchPicker';
import { freshShadowId, shadowTally } from '../../lib/shadowMerge';
import { isIdenticalShadow, pruneMerge, shadowTestedCount } from '../../lib/shadowRuns';
import { StepMergeEditor } from './StepShadowMergeEditor';
import { MAX_SHADOWS, stepCanCarryShadows, useVersions, type Versions } from './shadowEditorModel';
import { shadowSkillIds, stepMainAgent } from './shadowRowModel';
import type {
  ModelTiersConfig, Project, QuickPrompt, ShadowAgent, ShadowBudget, ShadowMerge, ShadowPrompt, Skill, StepShadow, WorkflowStep,
} from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

interface Props {
  step: WorkflowStep;
  /**
   * The step's shadows after an edit. The second argument is given only when the
   * edit also changes the step's merge (KT-1544): the new merge, or `null` to
   * turn it off.
   */
  onChange: (shadows: StepShadow[], merge?: ShadowMerge | null) => void;
  projects: readonly Project[];
  skills: readonly Skill[];
  quickPrompts: readonly QuickPrompt[];
  /** The agents and connections a shadow may run on (KT-1885); every agent Kronn knows when absent. */
  agentChoices?: readonly AgentSwitchTarget[];
  /** The agent families' configured models, to name a default model. */
  modelTiers?: ModelTiersConfig | null;
  t: Translate;
  /** Opened from a link to the shadow settings: start expanded, scrolled into view. */
  focusOnOpen?: boolean;
  /** Called once the section has been brought into view. */
  onFocused?: () => void;
  /** Distinguishes the sections of several steps in the page. */
  testId?: string;
}

const SHADOW_ID = /^[a-z0-9][a-z0-9._-]{0,39}$/;

const positive = (text: string): number | undefined => {
  const n = Number(text);
  return Number.isFinite(n) && n > 0 ? n : undefined;
};

/** What the step loads, readable: « v3 épinglée », « dernière (v5) ». */
function normalLabel(ref: number | null, latest: number | undefined, t: Translate): string {
  if (ref != null) return t('wf.shadow.pinnedVersion', ref);
  return latest != null ? t('wf.shadow.latestVersion', latest) : t('wf.shadow.latestVersionUnknown');
}

const DROP = 'drop';

function VersionRow({ kind, name, normalRef, current, dropped = false, canDrop = false, versions, onPick, testId, t }: {
  kind: string;
  name: string;
  /** The version the step pins, or null for « the latest ». */
  normalRef: number | null;
  current: number | null;
  dropped?: boolean;
  canDrop?: boolean;
  versions: Versions;
  onPick: (n: number | typeof DROP | null) => void;
  testId: string;
  t: Translate;
}) {
  const latest = versions.list[0];
  const sameAsStep = current != null && (normalRef ?? latest) === current;
  return (
    <div className="wf-shadow-version" data-testid={testId}>
      <span className="wf-shadow-version-name">
        <span className="wf-shadow-version-kind">{kind}</span>
        {name}
      </span>
      <span className="wf-shadow-version-normal">
        {t('wf.shadow.normalLoads', normalLabel(normalRef, latest, t))}
      </span>
      <label className="wf-shadow-version-pick">
        <span>{t('wf.shadow.shadowLoads')}</span>
        <select
          className="wf-shadow-select"
          value={dropped ? DROP : current ?? ''}
          disabled={versions.state === 'loading'}
          aria-label={t('wf.shadow.versionAria', name)}
          onChange={e => onPick(e.target.value === DROP ? DROP : e.target.value ? Number(e.target.value) : null)}
        >
          <option value="">
            {versions.state === 'loading' ? t('common.loading')
              : versions.state === 'error' ? t('wf.shadow.historyUnavailable')
                : t('wf.shadow.sameAsNormal')}
          </option>
          {canDrop && <option value={DROP}>{t('wf.shadow.dropSkill')}</option>}
          {current != null && !versions.list.includes(current) && <option value={current}>v{current}</option>}
          {versions.list.map(n => (
            <option key={n} value={n}>{n === latest ? t('wf.shadow.optionLatest', n) : `v${n}`}</option>
          ))}
        </select>
      </label>
      {sameAsStep && <span className="wf-shadow-same">{t('wf.shadow.sameVersionWarning')}</span>}
    </div>
  );
}

function SkillRow({ base, name, normalRef, shadow, update, t }: { base: string; name: string; normalRef: number | null; shadow: StepShadow; update: (s: StepShadow) => void; t: Translate }) {
  const versions = useVersions(() => skillsApi.history(base), base);
  const chosen = shadow.skills?.[base];
  const dropped = chosen === '';
  const current = chosen && skillBaseId(chosen) === base ? skillPinnedVersion(chosen) : null;
  return (
    <VersionRow
      kind={t('pages.resources.skill')}
      name={name}
      normalRef={normalRef}
      current={current}
      dropped={dropped}
      canDrop
      versions={versions}
      testId={`wf-shadow-skill-${base}`}
      t={t}
      onPick={n => {
        const skills = { ...(shadow.skills ?? {}) };
        if (n == null) delete skills[base];
        else if (n === DROP) skills[base] = '';
        else skills[base] = `${base}@${n}`;
        update({ ...shadow, skills });
      }}
    />
  );
}

function QuickPromptRow({ id, name, normalRef, shadow, update, t }: { id: string; name: string; normalRef: number | null; shadow: StepShadow; update: (s: StepShadow) => void; t: Translate }) {
  const versions = useVersions(() => quickPromptsApi.history(id), id);
  return (
    <VersionRow
      kind={t('pages.resources.quickPrompt')}
      name={name}
      normalRef={normalRef}
      current={shadow.quick_prompt_versions?.[id] ?? null}
      versions={versions}
      testId={`wf-shadow-qp-${id}`}
      t={t}
      onPick={n => {
        const quick_prompt_versions = { ...(shadow.quick_prompt_versions ?? {}) };
        if (n == null || n === DROP) delete quick_prompt_versions[id]; else quick_prompt_versions[id] = n;
        update({ ...shadow, quick_prompt_versions });
      }}
    />
  );
}

/** What the agent picker of a shadow needs from the page. */
interface AgentContext {
  main: MainAgent;
  choices?: readonly AgentSwitchTarget[];
  modelTiers?: ModelTiersConfig | null;
}

/**
 * The agent a shadow runs its step on (KT-1885): the shared agent picker in its
 * « exact model » mode — an agent and a model id (the catalogue's, or one typed
 * in its search), or « = principal »; the step's own model is greyed out.
 */
function AgentFields({ value, onChange, testId, context, t }: {
  value: ShadowAgent | undefined;
  onChange: (next: ShadowAgent | undefined) => void;
  testId: string;
  context: AgentContext;
  t: Translate;
}) {
  return (
    <span className="wf-shadow-agent-fields">
      <AgentSwitchPicker
        currentAgent={context.main.agent}
        availableAgents={[]}
        availableTargets={context.choices?.length ? [...context.choices] : undefined}
        modelTiers={context.modelTiers}
        variant="field"
        title={t('wf.shadow.agentTitle')}
        ariaLabel={t('wf.shadow.agentTitle')}
        exactModel={{ value, onChange, main: context.main, testId: `${testId}-agent` }}
      />
    </span>
  );
}

function ShadowEditor({ shadow, shadows, onChange, onRemove, step, projects, skills, quickPrompts, agentContext, t }: {
  shadow: StepShadow;
  shadows: readonly StepShadow[];
  onChange: (next: StepShadow) => void;
  onRemove: () => void;
  step: WorkflowStep;
  projects: readonly Project[];
  skills: readonly Skill[];
  quickPrompts: readonly QuickPrompt[];
  agentContext: AgentContext;
  t: Translate;
}) {
  const projectIds = shadow.project_ids ?? [];
  const idTaken = shadows.some(other => other !== shadow && other.id === shadow.id);
  const idInvalid = !SHADOW_ID.test(shadow.id);
  const budget: ShadowBudget = shadow.budget ?? {};
  const setBudget = (next: ShadowBudget) => onChange({ ...shadow, budget: next.max_tokens || next.max_duration_secs || next.max_cost_usd ? next : undefined });
  const prompt = shadow.prompt;
  const promptKind = prompt?.type ?? 'none';
  const testId = `wf-shadow-${shadow.id}`;
  const setPrompt = (next: ShadowPrompt | undefined) => onChange({ ...shadow, prompt: next });
  const stepQp = step.quick_prompt_id ? quickPrompts.find(qp => qp.id === step.quick_prompt_id) : undefined;
  // KT-1946 — the skills this shadow loads: another Quick Prompt brings its own.
  const stepSkills = shadowSkillIds(step, shadow, { quickPrompts });
  const customSkills = [...new Map(stepSkills.filter(id => skillBaseId(id).startsWith('custom-'))
    .map(id => [skillBaseId(id), skillPinnedVersion(id)] as const))];
  const unversioned = [...new Set(stepSkills.filter(id => !skillBaseId(id).startsWith('custom-')))];

  return (
    <div className="wf-shadow-item" data-testid={testId} data-identical={isIdenticalShadow(shadow)}>
      <div className="wf-shadow-item-head">
        <label className="wf-shadow-item-field">
          <span className="text-muted">{t('wf.shadow.id')}</span>
          <input
            className="wf-shadow-input"
            data-testid={`${testId}-id`}
            aria-invalid={idTaken || idInvalid}
            value={shadow.id}
            onChange={e => onChange({ ...shadow, id: e.target.value })}
          />
        </label>
        <label className="wf-shadow-item-field">
          <span className="text-muted">{t('wf.shadow.label')}</span>
          <input
            className="wf-shadow-input"
            data-testid={`${testId}-label`}
            value={shadow.label}
            placeholder={shadow.id}
            onChange={e => onChange({ ...shadow, label: e.target.value })}
          />
        </label>
        <label className="wf-shadow-item-field wf-shadow-item-enabled">
          <input
            type="checkbox"
            data-testid={`${testId}-enabled`}
            checked={shadow.enabled !== false}
            onChange={e => onChange({ ...shadow, enabled: e.target.checked })}
          />
          <span>{t('wf.shadow.enabled')}</span>
        </label>
        <button type="button" className="wf-small-btn wf-shadow-item-remove" data-testid={`${testId}-remove`} aria-label={t('wf.shadow.remove', shadow.label || shadow.id)} onClick={onRemove}>
          <Trash2 size={11} aria-hidden /> {t('wf.shadow.removeShort')}
        </button>
      </div>
      {(idTaken || idInvalid) && <p className="wf-shadow-warning" role="alert"><AlertTriangle size={12} /> {t(idTaken ? 'wf.shadow.idTaken' : 'wf.shadow.idInvalid')}</p>}
      <p className="text-muted wf-retention-help" data-testid={`${testId}-kind`}>
        <Info size={11} /> {t(isIdenticalShadow(shadow) ? 'wf.shadow.identicalHint' : 'wf.shadow.changesHint')}
      </p>

      <p className="wf-shadow-subtitle">{t('wf.shadow.agentTitle')}</p>
      <div className="wf-shadow-item-row">
        <AgentFields value={shadow.agent} onChange={agent => onChange({ ...shadow, agent })} testId={testId} context={agentContext} t={t} />
      </div>

      <p className="wf-shadow-subtitle">{t('wf.shadow.promptTitle')}</p>
      <div className="wf-shadow-item-row">
        <select
          className="wf-shadow-select"
          data-testid={`${testId}-prompt-kind`}
          aria-label={t('wf.shadow.promptTitle')}
          value={promptKind}
          onChange={e => {
            const kind = e.target.value;
            if (kind === 'none') setPrompt(undefined);
            else if (kind === 'inline') setPrompt({ type: 'inline', template: prompt?.type === 'inline' ? prompt.template : '' });
            else setPrompt({ type: 'quick_prompt', id: prompt?.type === 'quick_prompt' ? prompt.id : quickPrompts[0]?.id ?? '' });
          }}
        >
          <option value="none">{t('wf.shadow.promptOwn')}</option>
          <option value="quick_prompt">{t('wf.shadow.promptQuick')}</option>
          <option value="inline">{t('wf.shadow.promptInline')}</option>
        </select>
        {prompt?.type === 'quick_prompt' && (
          <>
            <select
              className="wf-shadow-select"
              data-testid={`${testId}-prompt-qp`}
              aria-label={t('pages.resources.quickPrompt')}
              value={prompt.id}
              onChange={e => setPrompt({ type: 'quick_prompt', id: e.target.value, version: undefined })}
            >
              {!quickPrompts.some(qp => qp.id === prompt.id) && <option value={prompt.id}>{prompt.id}</option>}
              {quickPrompts.map(qp => <option key={qp.id} value={qp.id}>{qp.name}</option>)}
            </select>
            <input
              className="wf-shadow-input wf-shadow-number"
              type="number"
              min={1}
              data-testid={`${testId}-prompt-version`}
              aria-label={t('wf.shadow.promptVersion')}
              placeholder={t('wf.shadow.promptVersion')}
              value={prompt.version ?? ''}
              onChange={e => setPrompt({ type: 'quick_prompt', id: prompt.id, version: positive(e.target.value) })}
            />
          </>
        )}
      </div>
      {prompt?.type === 'inline' && (
        <textarea
          className="wf-shadow-input wf-shadow-textarea"
          data-testid={`${testId}-prompt-inline`}
          aria-label={t('wf.shadow.promptInline')}
          rows={4}
          value={prompt.template}
          onChange={e => setPrompt({ type: 'inline', template: e.target.value })}
        />
      )}

      <p className="wf-shadow-subtitle">{t('wf.shadow.versionsTitle')}</p>
      {customSkills.length === 0 && !step.quick_prompt_id && <p className="text-muted wf-retention-help">{t('wf.shadow.noVersions')}</p>}
      {step.quick_prompt_id && (
        <QuickPromptRow
          id={step.quick_prompt_id}
          name={stepQp?.name ?? step.quick_prompt_id}
          normalRef={step.quick_prompt_version ?? null}
          shadow={shadow}
          update={onChange}
          t={t}
        />
      )}
      {customSkills.map(([base, pinned]) => (
        <SkillRow key={base} base={base} name={skills.find(skill => skill.id === base)?.name ?? base} normalRef={pinned} shadow={shadow} update={onChange} t={t} />
      ))}
      {unversioned.length > 0 && (
        <p className="text-muted wf-retention-help">{t('wf.shadow.unversioned', unversioned.map(id => skills.find(s => s.id === id)?.name ?? id).join(', '))}</p>
      )}

      {projects.length > 0 && (
        <div className="wf-shadow-projects">
          <span className="text-muted">{projectIds.length === 0 ? t('wf.shadow.allProjects') : t('wf.shadow.someProjects')}</span>
          <div className="wf-shadow-chips">
            {projects.map(project => {
              const on = projectIds.includes(project.id);
              return (
                <button
                  key={project.id}
                  type="button"
                  className="wf-chip"
                  data-selected={on}
                  aria-pressed={on}
                  onClick={() => onChange({ ...shadow, project_ids: on ? projectIds.filter(id => id !== project.id) : [...projectIds, project.id] })}
                >
                  {project.name}
                </button>
              );
            })}
          </div>
        </div>
      )}

      <p className="wf-shadow-subtitle">{t('wf.shadow.limitsTitle')}</p>
      <div className="wf-shadow-item-row wf-shadow-item-limits">
        <label className="wf-shadow-item-field">
          <span className="text-muted">{t('wf.shadow.sample')}</span>
          <input
            className="wf-shadow-input wf-shadow-number"
            type="number"
            min={1}
            data-testid={`${testId}-sample`}
            placeholder="1"
            value={shadow.sample_every ?? ''}
            onChange={e => onChange({ ...shadow, sample_every: positive(e.target.value) })}
          />
        </label>
        <label className="wf-shadow-item-field">
          <span className="text-muted">{t('wf.shadow.budgetTokens')}</span>
          <input
            className="wf-shadow-input wf-shadow-number"
            type="number"
            min={1}
            data-testid={`${testId}-budget-tokens`}
            value={budget.max_tokens ?? ''}
            onChange={e => setBudget({ ...budget, max_tokens: positive(e.target.value) })}
          />
        </label>
        <label className="wf-shadow-item-field">
          <span className="text-muted">{t('wf.shadow.budgetMinutes')}</span>
          <input
            className="wf-shadow-input wf-shadow-number"
            type="number"
            min={1}
            data-testid={`${testId}-budget-minutes`}
            value={budget.max_duration_secs ? Math.ceil(budget.max_duration_secs / 60) : ''}
            onChange={e => { const minutes = positive(e.target.value); setBudget({ ...budget, max_duration_secs: minutes ? Math.round(minutes * 60) : undefined }); }}
          />
        </label>
        <label className="wf-shadow-item-field">
          <span className="text-muted">{t('wf.shadow.budgetCost')}</span>
          <input
            className="wf-shadow-input wf-shadow-number"
            type="number"
            min={0}
            step="0.01"
            data-testid={`${testId}-budget-cost`}
            value={budget.max_cost_usd ?? ''}
            onChange={e => setBudget({ ...budget, max_cost_usd: positive(e.target.value) })}
          />
        </label>
      </div>
    </div>
  );
}

export function StepShadowsEditor({ step, onChange, projects, skills, quickPrompts, agentChoices, modelTiers, t, focusOnOpen = false, onFocused, testId = 'wf-step-shadows' }: Props) {
  const agentContext: AgentContext = { main: stepMainAgent(step), choices: agentChoices, modelTiers };
  const shadows = step.shadows ?? [];
  const [expanded, setExpanded] = useState(shadows.length > 0 || focusOnOpen);
  const toggleRef = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!focusOnOpen) return;
    toggleRef.current?.scrollIntoView?.({ block: 'start', behavior: 'smooth' });
    toggleRef.current?.focus({ preventScroll: true });
    onFocused?.();
    // `onFocused` is rebuilt each render; the focus request is the trigger.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusOnOpen]);
  const tested = useMemo(() => shadowTestedCount([step]), [step]);
  const eligible = stepCanCarryShadows(step);
  // A merge names shadows by id: it follows a rename and forgets a removal.
  const change = (next: StepShadow[]) => {
    const merge = pruneMerge(step.shadow_merge, shadows, next);
    if (merge === undefined) onChange(next);
    else onChange(next, merge);
  };
  const setShadow = (index: number, next: StepShadow) => change(shadows.map((shadow, i) => (i === index ? next : shadow)));

  return (
    <div className="wf-section wf-retention-card wf-shadow-card" data-testid={testId}>
      <button ref={toggleRef} type="button" className="wf-retention-toggle" aria-expanded={expanded} onClick={() => setExpanded(e => !e)}>
        {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <FlaskConical size={14} />
        <span>{t('wf.shadow.title')}</span>
        <span className="text-muted wf-retention-summary">
          {shadows.length > 0 ? t('wf.shadow.summary', shadows.length, tested) : t('wf.shadow.summaryNone')}
        </span>
      </button>
      {expanded && (
        <div className="wf-retention-body">
          <p className="text-muted wf-retention-help"><Info size={11} /> {t('wf.shadow.description')}</p>
          {!eligible && (
            <p className="wf-shadow-warning" role="note" data-testid={`${testId}-ineligible`}>
              <AlertTriangle size={12} /> {t(step.external_effect ? 'wf.shadow.outsideKronn' : 'wf.shadow.onlyAgent')}
            </p>
          )}
          {shadows.map((shadow, index) => (
            <ShadowEditor
              key={index}
              shadow={shadow}
              shadows={shadows}
              onChange={next => setShadow(index, next)}
              onRemove={() => change(shadows.filter((_, i) => i !== index))}
              step={step}
              projects={projects}
              skills={skills}
              quickPrompts={quickPrompts}
              agentContext={agentContext}
              t={t}
            />
          ))}
          {shadows.length === 0 && eligible && <p className="text-muted wf-retention-help" data-testid={`${testId}-none`}>{t('wf.shadow.none')}</p>}
          <button
            type="button"
            className="wf-small-btn"
            data-testid={`${testId}-add`}
            disabled={!eligible || shadows.length >= MAX_SHADOWS}
            onClick={() => onChange([...shadows, { id: freshShadowId(shadows), label: '', enabled: true }])}
          >
            <Plus size={11} aria-hidden /> {t('wf.shadow.add')}
          </button>
          {eligible && shadows.length > 0 && (
            <StepMergeEditor
              shadows={shadows}
              merge={step.shadow_merge}
              onChange={merge => onChange(shadows, merge)}
              agentFields={(value, onValue, id) => <AgentFields value={value} onChange={onValue} testId={id} context={agentContext} t={t} />}
              testId={testId}
              t={t}
            />
          )}
          <p className="text-muted wf-retention-help" data-testid={`${testId}-workdir`}><Info size={11} /> {t('wf.shadow.sharedWorkdir')}</p>
          <p className="text-muted wf-retention-help"><Info size={11} /> {t('wf.shadow.cost')}</p>
        </div>
      )}
    </div>
  );
}

/** The step shadows on the workflow page, and the way to their settings. */
export function ShadowStatusButton({ steps, projects, onOpen, t }: {
  steps: readonly WorkflowStep[] | null | undefined;
  projects: readonly Project[];
  onOpen: () => void;
  t: Translate;
}) {
  const shadowed = (steps ?? []).filter(step => (step.shadows ?? []).some(shadow => shadow.enabled !== false));
  const count = shadowed.reduce((total, step) => total + (step.shadows ?? []).filter(shadow => shadow.enabled !== false).length, 0);
  const active = count > 0;
  // Every project when some shadow covers them all; else the ones the shadows name.
  const all = shadowed.flatMap(step => step.shadows ?? []).filter(shadow => shadow.enabled !== false);
  const projectIds = all.some(shadow => (shadow.project_ids ?? []).length === 0)
    ? []
    : [...new Set(all.flatMap(shadow => shadow.project_ids ?? []))];
  const projectNames = projectIds.map(id => projects.find(p => p.id === id)?.name ?? id).join(', ');
  // KT-1696 — « ⑂ 2 shadows · 1 step · ⊕ merge »: what is doubled, and whether it is merged.
  const tally = shadowTally(steps);
  const label = active
    ? [
      t(tally.shadows === 1 ? 'wf.shadow.entryShadowsOne' : 'wf.shadow.entryShadowsMany', tally.shadows),
      t(tally.steps === 1 ? 'wf.shadow.entryStepsOne' : 'wf.shadow.entryStepsMany', tally.steps),
      ...(tally.merged ? [t('wf.shadow.entryMerge')] : []),
    ].join(' · ')
    : t('wf.shadow.entryOff');
  const title = active
    ? t('wf.shadow.entryTitleOn', projectIds.length === 0 ? t('wf.shadow.entryAllProjects') : projectNames)
    : t('wf.shadow.entryTitleOff');
  return (
    <button
      type="button"
      className="wf-small-btn wf-shadow-entry"
      data-active={active}
      data-testid="wf-shadow-entry"
      title={title}
      aria-label={`${label}. ${title}`}
      onClick={onOpen}
    >
      {!active && <FlaskConical size={10} aria-hidden />} {label}
    </button>
  );
}

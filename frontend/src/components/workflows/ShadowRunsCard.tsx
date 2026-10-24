/**
 * KT-1449 — the workflow's shadow variant: each normal run of the chosen
 * projects also starts a shadow run on the same inputs, with other skill and
 * Quick Prompt versions; steps acting outside Kronn and Gates are skipped.
 */
import { useEffect, useMemo, useState } from 'react';
import { AlertTriangle, ChevronDown, ChevronRight, FlaskConical, Info } from 'lucide-react';
import { skills as skillsApi, quickPrompts as quickPromptsApi } from '../../lib/api';
import { skillBaseId, skillPinnedVersion } from '../../lib/automationSkills';
import type { Project, QuickPrompt, Skill, WorkflowShadow, WorkflowStep } from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

interface Props {
  value: WorkflowShadow | null | undefined;
  onChange: (next: WorkflowShadow | null) => void;
  steps: readonly WorkflowStep[];
  projects: readonly Project[];
  skills: readonly Skill[];
  quickPrompts: readonly QuickPrompt[];
  t: Translate;
}

type Versions = { state: 'loading' | 'error' | 'ready'; list: number[] };

/** Versions of one skill or QP, newest first. */
function useVersions(load: () => Promise<{ version_index: number }[] | null | undefined>, key: string): Versions {
  const [loaded, setLoaded] = useState<{ key: string; versions: Versions } | null>(null);
  useEffect(() => {
    let cancelled = false;
    load()
      .then(list => {
        const sorted = (list ?? []).map(v => v.version_index).sort((a, b) => b - a);
        if (!cancelled) setLoaded({ key, versions: { state: 'ready', list: sorted } });
      })
      .catch(() => { if (!cancelled) setLoaded({ key, versions: { state: 'error', list: [] } }); });
    return () => { cancelled = true; };
    // `load` is rebuilt each render; the key names what it loads.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return loaded?.key === key ? loaded.versions : { state: 'loading', list: [] };
}

/** What the normal run loads, readable: « v3 épinglée », « dernière (v5) ». */
function normalLabel(refs: readonly (number | null)[], latest: number | undefined, t: Translate): string {
  const labels = [...new Set(refs.map(n => (n == null
    ? (latest != null ? t('wf.shadow.latestVersion', latest) : t('wf.shadow.latestVersionUnknown'))
    : t('wf.shadow.pinnedVersion', n))))];
  return labels.join(' · ');
}

function VersionRow({ kind, name, normalRefs, current, versions, onPick, testId, t }: {
  kind: string;
  name: string;
  /** Per step: the pinned version, or null for « the latest ». */
  normalRefs: readonly (number | null)[];
  current: number | null;
  versions: Versions;
  onPick: (n: number | null) => void;
  testId: string;
  t: Translate;
}) {
  const latest = versions.list[0];
  const sameAsNormal = current != null && normalRefs.every(n => (n ?? latest) === current);
  return (
    <div className="wf-shadow-version" data-testid={testId}>
      <span className="wf-shadow-version-name">
        <span className="wf-shadow-version-kind">{kind}</span>
        {name}
      </span>
      <span className="wf-shadow-version-normal">
        {t('wf.shadow.normalLoads', normalLabel(normalRefs, latest, t))}
      </span>
      <label className="wf-shadow-version-pick">
        <span>{t('wf.shadow.shadowLoads')}</span>
        <select
          className="wf-shadow-select"
          value={current ?? ''}
          disabled={versions.state === 'loading'}
          aria-label={t('wf.shadow.versionAria', name)}
          onChange={e => onPick(e.target.value ? Number(e.target.value) : null)}
        >
          <option value="">
            {versions.state === 'loading' ? t('common.loading')
              : versions.state === 'error' ? t('wf.shadow.historyUnavailable')
                : t('wf.shadow.sameAsNormal')}
          </option>
          {current != null && !versions.list.includes(current) && <option value={current}>v{current}</option>}
          {versions.list.map(n => (
            <option key={n} value={n}>{n === latest ? t('wf.shadow.optionLatest', n) : `v${n}`}</option>
          ))}
        </select>
      </label>
      {sameAsNormal && <span className="wf-shadow-same">{t('wf.shadow.sameVersionWarning')}</span>}
    </div>
  );
}

function SkillRow({ base, name, normalRefs, shadow, update, t }: { base: string; name: string; normalRefs: (number | null)[]; shadow: WorkflowShadow; update: (s: WorkflowShadow) => void; t: Translate }) {
  const versions = useVersions(() => skillsApi.history(base), base);
  const chosen = shadow.skills?.[base];
  const current = chosen && skillBaseId(chosen) === base ? skillPinnedVersion(chosen) : null;
  return (
    <VersionRow
      kind={t('pages.resources.skill')}
      name={name}
      normalRefs={normalRefs}
      current={current}
      versions={versions}
      testId={`wf-shadow-skill-${base}`}
      t={t}
      onPick={n => {
        const skills = { ...(shadow.skills ?? {}) };
        if (n == null) delete skills[base]; else skills[base] = `${base}@${n}`;
        update({ ...shadow, skills });
      }}
    />
  );
}

function QuickPromptRow({ id, name, shadow, update, t }: { id: string; name: string; shadow: WorkflowShadow; update: (s: WorkflowShadow) => void; t: Translate }) {
  const versions = useVersions(() => quickPromptsApi.history(id), id);
  return (
    <VersionRow
      kind={t('pages.resources.quickPrompt')}
      name={name}
      normalRefs={[null]}
      current={shadow.quick_prompt_versions?.[id] ?? null}
      versions={versions}
      testId={`wf-shadow-qp-${id}`}
      t={t}
      onPick={n => {
        const quick_prompt_versions = { ...(shadow.quick_prompt_versions ?? {}) };
        if (n == null) delete quick_prompt_versions[id]; else quick_prompt_versions[id] = n;
        update({ ...shadow, quick_prompt_versions });
      }}
    />
  );
}

export function ShadowRunsCard({ value, onChange, steps, projects, skills, quickPrompts, t }: Props) {
  const shadow: WorkflowShadow = value ?? { enabled: false };
  const [expanded, setExpanded] = useState(Boolean(value?.enabled));
  /** Per step, the skills it loads: its own, else its Quick Prompt's. */
  const stepSkills = useMemo(() => steps.map(step => (step.skill_ids?.length
    ? step.skill_ids
    : quickPrompts.find(qp => qp.id === step.quick_prompt_id)?.skill_ids ?? [])), [steps, quickPrompts]);
  /** Each custom skill the steps load → what each step pins (null = latest). */
  const usedSkills = useMemo(() => {
    const map = new Map<string, (number | null)[]>();
    for (const id of stepSkills.flat()) {
      const base = skillBaseId(id);
      if (base.startsWith('custom-')) map.set(base, [...(map.get(base) ?? []), skillPinnedVersion(id)]);
    }
    return map;
  }, [stepSkills]);
  const unversioned = useMemo(() => [...new Set(stepSkills.flat().filter(id => !skillBaseId(id).startsWith('custom-')))], [stepSkills]);
  const usedQuickPrompts = useMemo(() => [...new Set(steps.map(step => step.quick_prompt_id).filter((id): id is string => !!id))], [steps]);
  const skipped = steps.filter(step => step.external_effect || step.step_type?.type === 'Gate').map(step => step.name);
  const update = (next: WorkflowShadow) => onChange(next);
  const projectIds = shadow.project_ids ?? [];
  const tested = Object.keys(shadow.skills ?? {}).length + Object.keys(shadow.quick_prompt_versions ?? {}).length;
  const hasVersions = usedSkills.size > 0 || usedQuickPrompts.length > 0;

  return (
    <div className="wf-section wf-retention-card wf-shadow-card" data-testid="wf-shadow-card">
      <button type="button" className="wf-retention-toggle" aria-expanded={expanded} onClick={() => setExpanded(e => !e)}>
        {expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <FlaskConical size={14} />
        <span>{t('wf.shadow.title')}</span>
        <span className="text-muted wf-retention-summary">
          {shadow.enabled ? t('wf.shadow.summaryOn', tested) : t('wf.shadow.summaryOff')}
        </span>
      </button>
      {expanded && (
        <div className="wf-retention-body">
          <p className="text-muted wf-retention-help"><Info size={11} /> {t('wf.shadow.description')}</p>
          <label className="wf-shadow-row wf-shadow-toggle">
            <input
              type="checkbox"
              data-testid="wf-shadow-enabled"
              checked={shadow.enabled}
              onChange={e => update({ ...shadow, enabled: e.target.checked })}
            />
            <span>{t('wf.shadow.enable')}</span>
          </label>
          {shadow.enabled && (
            skipped.length > 0
              ? <p className="text-muted wf-retention-help" data-testid="wf-shadow-skipped"><Info size={11} /> {t('wf.shadow.skippedSteps', skipped.join(', '))}</p>
              : <p className="wf-shadow-warning" role="note" data-testid="wf-shadow-nothing-skipped"><AlertTriangle size={12} /> {t('wf.shadow.nothingSkipped')}</p>
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
                      onClick={() => update({ ...shadow, project_ids: on ? projectIds.filter(id => id !== project.id) : [...projectIds, project.id] })}
                    >
                      {project.name}
                    </button>
                  );
                })}
              </div>
            </div>
          )}
          <p className="wf-shadow-subtitle">{t('wf.shadow.versionsTitle')}</p>
          {!hasVersions && <p className="text-muted wf-retention-help">{t('wf.shadow.noVersions')}</p>}
          {usedQuickPrompts.map(id => (
            <QuickPromptRow key={id} id={id} name={quickPrompts.find(qp => qp.id === id)?.name ?? id} shadow={shadow} update={update} t={t} />
          ))}
          {[...usedSkills].map(([base, refs]) => (
            <SkillRow key={base} base={base} name={skills.find(skill => skill.id === base)?.name ?? base} normalRefs={refs} shadow={shadow} update={update} t={t} />
          ))}
          {unversioned.length > 0 && (
            <p className="text-muted wf-retention-help">{t('wf.shadow.unversioned', unversioned.map(id => skills.find(s => s.id === id)?.name ?? id).join(', '))}</p>
          )}
          <p className="text-muted wf-retention-help" data-testid="wf-shadow-shared-paths"><Info size={11} /> {t('wf.shadow.sharedPaths')}</p>
          <p className="text-muted wf-retention-help"><Info size={11} /> {t('wf.shadow.cost')}</p>
        </div>
      )}
    </div>
  );
}

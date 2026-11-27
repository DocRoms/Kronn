/**
 * KT-1450, KT-1459 — the Quick Prompts and skills behind a Page: per step of a
 * workflow, the version its normal execution loads and the one each of its
 * shadows tests, their history, the promotion of a tested version to the step,
 * and the shadows themselves: test a version in a new shadow, change what a
 * shadow tests, remove a shadow.
 */
import { useCallback, useEffect, useState } from 'react';
import { Layers, FlaskConical, ArrowUpCircle, ArrowRight, CheckCircle2, RefreshCw } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { pages as pagesApi, workflows as workflowsApi, skills as skillsApi, quickPrompts as quickPromptsApi } from '../lib/api';
import { automationPath, workflowPath, workflowShadowSettingsPath } from '../lib/routes';
import { skillBaseId, skillPinnedVersion } from '../lib/automationSkills';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { AppLink } from './AppLink';
import { useConfirm } from './ui/useConfirm';
import SkillHistoryDrawer from './SkillHistoryDrawer';
import QPHistoryDrawer from './QPHistoryDrawer';
import { useVersions, MAX_SHADOWS } from './workflows/shadowEditorModel';
import type { LivePageResource, LivePageResourceUse, StepShadow, WorkflowStep } from '../types/generated';
import './LivePageResources.css';

type Translate = (key: string, ...args: (string | number)[]) => string;

interface Props {
  pageId: string;
  onNavigateWorkflow?: (workflowId: string) => void;
}

/** One step's use of a resource: what its normal execution loads, and what each shadow loads. */
interface StepUse {
  workflowId: string;
  workflowName: string;
  step: string;
  /** What the normal execution loads. */
  normal: string | null;
  /** What each enabled shadow of the step loads in place of it, in order. */
  shadows: { id: string; label: string; ref: string }[];
}

function stepUses(uses: readonly LivePageResourceUse[]): StepUse[] {
  return uses.map(use => ({
    workflowId: use.workflow_id,
    workflowName: use.workflow_name,
    step: use.step_name,
    normal: use.normal,
    shadows: (use.shadows ?? []).map(shadow => ({ id: shadow.shadow_id, label: shadow.label, ref: shadow.version })),
  }));
}

/** The version number a shadow loads, when it names one of this resource. */
function versionOf(resource: LivePageResource, ref: string | null): number | null {
  if (ref == null) return resource.latest_version;
  if (resource.kind === 'quick_prompt') return Number(ref.replace(/^v/, '')) || null;
  if (skillBaseId(ref) !== resource.id) return null;
  return skillPinnedVersion(ref) ?? resource.latest_version;
}

function refLabel(resource: LivePageResource, ref: string | null, t: Translate, side: 'normal' | 'shadow' = 'normal'): string {
  if (side === 'shadow' && ref === '') return t('pages.resources.dropped');
  if (resource.kind === 'quick_prompt') {
    return ref == null ? t('pages.resources.currentVersion', resource.latest_version ?? '?') : ref;
  }
  if (ref == null || skillBaseId(ref) !== resource.id) return ref ?? '';
  const pinned = skillPinnedVersion(ref);
  if (pinned != null) return side === 'shadow' ? `v${pinned}` : t('pages.resources.pinnedVersion', pinned);
  return resource.latest_version != null ? t('pages.resources.latestVersion', resource.latest_version) : t('pages.resources.unversioned');
}

export function LivePageResources({ pageId, onNavigateWorkflow }: Props) {
  const { t } = useT();
  const [resources, setResources] = useState<LivePageResource[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [promoting, setPromoting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);
  const { confirm, dialog: confirmDialog } = useConfirm();

  useEffect(() => {
    let cancelled = false;
    Promise.resolve().then(() => pagesApi.resources(pageId))
      .then(list => { if (!cancelled) { setResources(list ?? []); setLoadError(null); } })
      .catch(e => { if (!cancelled) setLoadError(String((e as Error).message ?? e)); });
    return () => { cancelled = true; };
  }, [pageId, reload]);

  const refresh = useCallback(() => setReload(n => n + 1), []);

  /** Saves the step's shadows as `change` leaves them: a person's edit, never an agent's. */
  const editShadows = useAsyncGuard(async (use: StepUse, change: (shadows: StepShadow[]) => StepShadow[], saved: string) => {
    setError(null);
    setDone(null);
    try {
      const workflow = await workflowsApi.get(use.workflowId);
      if (!workflow) throw new Error(t('pages.resources.stepGone', use.step));
      const steps: WorkflowStep[] = workflow.steps.map(step => (step.name === use.step ? { ...step, shadows: change(step.shadows ?? []) } : step));
      await workflowsApi.update(use.workflowId, { steps });
      setDone(saved);
      refresh();
    } catch (e) {
      setError(String((e as Error).message ?? e));
    }
  });

  const promote = useAsyncGuard(async (resource: LivePageResource, use: StepUse, label: string, shadowId: string) => {
    const isSkill = resource.kind === 'skill';
    // KT-1881 (KT-1456) — a ConfirmDialog in place of window.confirm.
    const confirmed = await confirm({
      title: t('pages.resources.promoteConfirmTitle', resource.name, label),
      message: isSkill
        ? t('pages.resources.promoteSkillConfirm', resource.name, label, use.workflowName, use.step)
        : t('pages.resources.promoteQpConfirm', resource.name, label, use.workflowName, use.step),
      confirmLabel: t('pages.resources.promote', label),
      tone: 'primary',
    });
    if (!confirmed) return;
    setPromoting(`${use.workflowId}:${use.step}:${resource.kind}:${resource.id}`);
    setError(null);
    setDone(null);
    try {
      await workflowsApi.promoteShadow(use.workflowId, { kind: resource.kind, id: resource.id, step: use.step, shadow: shadowId });
      setDone(isSkill
        ? t('pages.resources.promotedSkill', resource.name, use.workflowName, label)
        : t('pages.resources.promotedQp', resource.name, label));
      refresh();
    } catch (e) {
      setError(String((e as Error).message ?? e));
    } finally {
      setPromoting(null);
    }
  });

  if (loadError !== null && resources === null) {
    return (
      <div className="live-pages-resources" data-testid="live-page-resources-error" role="alert">
        <span className="live-pages-resources-error-line">
          <Layers size={13} aria-hidden /> {t('pages.resources.loadError')}
          <button type="button" className="live-pages-resource-retry" onClick={refresh}>
            <RefreshCw size={11} aria-hidden /> {t('pages.resources.retry')}
          </button>
        </span>
      </div>
    );
  }
  if (!resources || resources.length === 0) return null;
  const shadowed = resources.filter(r => r.uses.some(use => (use.shadows ?? []).length > 0)).length;
  const firstWorkflow = resources.flatMap(r => r.uses)[0];

  return (
    <details className="live-pages-resources" data-testid="live-page-resources" open={done !== null || error !== null || undefined}>
      <summary>
        <Layers size={13} aria-hidden />
        <strong>{t('pages.resources.title')}</strong>
        <span className="live-pages-resources-count">{resources.length}</span>
        {shadowed > 0 && (
          <span className="live-pages-resources-shadow"><FlaskConical size={11} aria-hidden /> {t('pages.resources.inShadow', shadowed)}</span>
        )}
      </summary>
      {error && <p className="live-pages-resources-error" role="alert">{error}</p>}
      {done && <p className="live-pages-resources-done" role="status" data-testid="live-page-resources-done"><CheckCircle2 size={12} aria-hidden /> {done}</p>}
      <ul className="live-pages-resources-list">
        {resources.map(resource => {
          const isSkill = resource.kind === 'skill';
          const editPath = automationPath({ tab: isSkill ? 'skills' : 'quickPrompts', resourceId: resource.id });
          return (
            <li key={`${resource.kind}:${resource.id}`} className="live-pages-resource" data-testid={`live-page-resource-${resource.id}`}>
              <div className="live-pages-resource-head">
                <span className="live-pages-resource-kind">{t(isSkill ? 'pages.resources.skill' : 'pages.resources.quickPrompt')}</span>
                <AppLink to={editPath} className="live-pages-resource-name">{resource.name}</AppLink>
                {resource.latest_version != null && (
                  <span className="live-pages-resource-version">{t('pages.resources.latest', resource.latest_version)}</span>
                )}
                <span className="live-pages-resource-history">
                  {isSkill && resource.id.startsWith('custom-') && (
                    <SkillHistoryDrawer skillId={resource.id} skillName={resource.name} refreshKey={String(reload)} onRestored={refresh} />
                  )}
                  {!isSkill && <QPHistoryDrawer qpId={resource.id} qpName={resource.name} refreshTrigger={reload} />}
                </span>
              </div>
              {stepUses(resource.uses).map(use => (
                <UseRow
                  key={`${use.workflowId}:${use.step}`}
                  resource={resource}
                  use={use}
                  busy={promoting === `${use.workflowId}:${use.step}:${resource.kind}:${resource.id}`}
                  onPromote={(label, shadowId) => { void promote(resource, use, label, shadowId); }}
                  onEditShadows={(change, saved) => { void editShadows(use, change, saved); }}
                  onNavigateWorkflow={onNavigateWorkflow}
                  t={t}
                />
              ))}
            </li>
          );
        })}
      </ul>
      {shadowed === 0 && firstWorkflow && (
        <p className="live-pages-resources-hint">
          <FlaskConical size={11} aria-hidden /> {t('pages.resources.shadowHint')}{' '}
          <AppLink to={workflowShadowSettingsPath(firstWorkflow.workflow_id)}>
            {t('pages.resources.shadowHintLink', firstWorkflow.workflow_name)}
          </AppLink>
        </p>
      )}
      {confirmDialog}
    </details>
  );
}

/** The shadow `shadowId` testing `version` of the resource (`null`: no longer testing it). */
function withVersion(shadows: StepShadow[], resource: LivePageResource, shadowId: string, version: number | 'drop' | null): StepShadow[] {
  return shadows.map(shadow => {
    if (shadow.id !== shadowId) return shadow;
    if (resource.kind === 'quick_prompt') {
      const quick_prompt_versions = { ...(shadow.quick_prompt_versions ?? {}) };
      if (typeof version === 'number') quick_prompt_versions[resource.id] = version; else delete quick_prompt_versions[resource.id];
      return { ...shadow, quick_prompt_versions };
    }
    const skills = { ...(shadow.skills ?? {}) };
    if (version === 'drop') skills[resource.id] = '';
    else if (version == null) delete skills[resource.id];
    else skills[resource.id] = `${resource.id}@${version}`;
    return { ...shadow, skills };
  });
}

function freshShadow(shadows: readonly StepShadow[]): string {
  for (let n = shadows.length + 1; ; n += 1) {
    if (!shadows.some(shadow => shadow.id === `shadow-${n}`)) return `shadow-${n}`;
  }
}

function ShadowControls({ resource, use, onEditShadows, t }: {
  resource: LivePageResource;
  use: StepUse;
  onEditShadows: (change: (shadows: StepShadow[]) => StepShadow[], saved: string) => void;
  t: Translate;
}) {
  const isSkill = resource.kind === 'skill';
  const versions = useVersions(
    () => (isSkill ? skillsApi.history(resource.id) : quickPromptsApi.history(resource.id)),
    `${resource.kind}:${resource.id}`,
  );
  const [newVersion, setNewVersion] = useState('');
  const options = versions.list.map(n => <option key={n} value={n}>{n === versions.list[0] ? t('wf.shadow.optionLatest', n) : `v${n}`}</option>);
  const key = `${resource.id}-${use.workflowId}-${use.step}`;
  return (
    <details className="live-pages-resource-lane-edit" data-testid={`live-page-resource-shadows-${key}`}>
      <summary>{t('pages.resources.shadowsEdit')}</summary>
      {use.shadows.map(shadow => {
        const current = versionOf(resource, shadow.ref);
        return (
          <div key={shadow.id} className="live-pages-resource-lane-row" data-testid={`live-page-resource-shadow-${resource.id}-${shadow.id}`}>
            <span className="live-pages-resource-lane-name">{shadow.label}</span>
            <select
              className="wf-shadow-select"
              aria-label={t('pages.resources.shadowVersion', shadow.label)}
              value={shadow.ref === '' ? 'drop' : current ?? ''}
              onChange={e => {
                // Read now: the controlled select is reset by the time the save runs.
                const picked = e.target.value;
                const version = picked === 'drop' ? 'drop' : picked ? Number(picked) : null;
                onEditShadows(shadows => withVersion(shadows, resource, shadow.id, version), t('pages.resources.shadowSaved', shadow.label));
              }}
            >
              {isSkill && <option value="drop">{t('pages.resources.dropped')}</option>}
              {current != null && !versions.list.includes(current) && <option value={current}>v{current}</option>}
              {options}
            </select>
            <button
              type="button"
              className="wf-small-btn"
              data-testid={`live-page-resource-shadow-remove-${resource.id}-${shadow.id}`}
              onClick={() => {
                if (!window.confirm(t('pages.resources.shadowRemoveConfirm', shadow.label, use.step))) return;
                onEditShadows(shadows => shadows.filter(other => other.id !== shadow.id), t('pages.resources.shadowRemoved', shadow.label));
              }}
            >
              {t('wf.shadow.removeShort')}
            </button>
          </div>
        );
      })}
      <div className="live-pages-resource-lane-row">
        <span className="live-pages-resource-lane-name">{t('pages.resources.shadowAdd')}</span>
        <select className="wf-shadow-select" aria-label={t('pages.resources.shadowAddVersion')} value={newVersion} onChange={e => setNewVersion(e.target.value)}>
          <option value="">{versions.state === 'loading' ? t('common.loading') : t('pages.resources.shadowAddVersion')}</option>
          {options}
        </select>
        <button
          type="button"
          className="wf-small-btn"
          data-testid={`live-page-resource-shadow-add-${key}`}
          disabled={!newVersion || use.shadows.length >= MAX_SHADOWS}
          onClick={() => {
            const n = Number(newVersion);
            onEditShadows(shadows => {
              const shadow: StepShadow = { id: freshShadow(shadows), label: `${resource.name} v${n}`, enabled: true };
              return withVersion([...shadows, shadow], resource, shadow.id, n);
            }, t('pages.resources.shadowCreated', `${resource.name} v${n}`));
            setNewVersion('');
          }}
        >
          {t('pages.resources.shadowAddButton')}
        </button>
      </div>
    </details>
  );
}

function UseRow({ resource, use, busy, onPromote, onEditShadows, onNavigateWorkflow, t }: {
  resource: LivePageResource;
  use: StepUse;
  busy: boolean;
  onPromote: (label: string, shadowId: string) => void;
  onEditShadows: (change: (shadows: StepShadow[]) => StepShadow[], saved: string) => void;
  onNavigateWorkflow?: (workflowId: string) => void;
  t: Translate;
}) {
  return (
    <div className="live-pages-resource-use" data-testid={`live-page-resource-use-${resource.id}-${use.workflowId}`}>
      <span className="live-pages-resource-where">
        <AppLink
          to={workflowPath(use.workflowId)}
          onNavigate={onNavigateWorkflow ? () => onNavigateWorkflow(use.workflowId) : undefined}
          className="live-pages-resource-workflow"
        >
          {use.workflowName}
        </AppLink>
        <span className="live-pages-resource-muted">{t('pages.resources.step', use.step)}</span>
      </span>
      <span className="live-pages-resource-lanes">
        <span className="live-pages-resource-lane">
          <span className="live-pages-resource-muted">{t('pages.resources.normal')}</span>
          {refLabel(resource, use.normal, t)}
        </span>
        {use.shadows.map(shadow => {
          const tested = shadow.ref ? versionOf(resource, shadow.ref) : null;
          const same = tested != null && versionOf(resource, use.normal) === tested;
          const label = tested != null ? `v${tested}` : refLabel(resource, shadow.ref, t, 'shadow');
          return (
            <span key={shadow.id} className="live-pages-resource-lane-group" data-testid={`live-page-resource-shadow-group-${resource.id}-${shadow.id}`}>
              <ArrowRight size={11} aria-hidden className="live-pages-resource-muted" />
              <span className="live-pages-resource-lane" data-lane="shadow">
                <span className="live-pages-resource-muted"><FlaskConical size={10} aria-hidden /> {shadow.label}</span>
                {refLabel(resource, shadow.ref, t, 'shadow')}
              </span>
              <span className="live-pages-resource-action">
                {!same && (
                  <button
                    type="button"
                    className="live-pages-resource-promote"
                    data-testid={`live-page-resource-promote-${resource.id}-${shadow.id}`}
                    disabled={busy}
                    title={t('pages.resources.promoteTitle', use.workflowName, use.step)}
                    onClick={() => onPromote(label, shadow.id)}
                  >
                    <ArrowUpCircle size={12} aria-hidden /> {t('pages.resources.promote', label)}
                  </button>
                )}
                {same && <span className="live-pages-resource-muted">{t('pages.resources.sameAsNormal')}</span>}
              </span>
            </span>
          );
        })}
      </span>
      <ShadowControls resource={resource} use={use} onEditShadows={onEditShadows} t={t} />
    </div>
  );
}

/**
 * KT-1450 — the Quick Prompts and skills behind a Page: per workflow, the
 * version its normal runs load and the one its shadow runs test, their
 * history, and the promotion of a tested version to the normal runs.
 */
import { useCallback, useEffect, useState } from 'react';
import { Layers, FlaskConical, ArrowUpCircle, ArrowRight, CheckCircle2, RefreshCw } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { pages as pagesApi, workflows as workflowsApi } from '../lib/api';
import { automationPath, workflowPath } from '../lib/routes';
import { skillBaseId, skillPinnedVersion } from '../lib/automationSkills';
import { useAsyncGuard } from '../hooks/useAsyncGuard';
import { AppLink } from './AppLink';
import SkillHistoryDrawer from './SkillHistoryDrawer';
import QPHistoryDrawer from './QPHistoryDrawer';
import type { LivePageResource, LivePageResourceUse } from '../types/generated';
import './LivePageResources.css';

type Translate = (key: string, ...args: (string | number)[]) => string;

interface Props {
  pageId: string;
  onNavigateWorkflow?: (workflowId: string) => void;
}

/** One workflow's use of a resource: its steps, and what each lane loads. */
interface WorkflowUse {
  workflowId: string;
  workflowName: string;
  steps: string[];
  /** Per step, what its normal run loads. */
  normal: { step: string; ref: string | null }[];
  shadow: string | null;
}

function byWorkflow(uses: readonly LivePageResourceUse[]): WorkflowUse[] {
  const groups = new Map<string, WorkflowUse>();
  for (const use of uses) {
    const group = groups.get(use.workflow_id) ?? { workflowId: use.workflow_id, workflowName: use.workflow_name, steps: [], normal: [], shadow: null };
    group.steps.push(use.step_name);
    group.normal.push({ step: use.step_name, ref: use.normal });
    group.shadow = group.shadow ?? use.shadow;
    groups.set(use.workflow_id, group);
  }
  return [...groups.values()];
}

/** The version number a lane loads, when it names one of this resource. */
function versionOf(resource: LivePageResource, ref: string | null): number | null {
  if (ref == null) return resource.latest_version;
  if (resource.kind === 'quick_prompt') return Number(ref.replace(/^v/, '')) || null;
  if (skillBaseId(ref) !== resource.id) return null;
  return skillPinnedVersion(ref) ?? resource.latest_version;
}

function laneLabel(resource: LivePageResource, ref: string | null, t: Translate, lane: 'normal' | 'shadow' = 'normal'): string {
  if (resource.kind === 'quick_prompt') {
    return ref == null ? t('pages.resources.currentVersion', resource.latest_version ?? '?') : ref;
  }
  if (ref == null || skillBaseId(ref) !== resource.id) return ref ?? '';
  const pinned = skillPinnedVersion(ref);
  if (pinned != null) return lane === 'shadow' ? `v${pinned}` : t('pages.resources.pinnedVersion', pinned);
  return resource.latest_version != null ? t('pages.resources.latestVersion', resource.latest_version) : t('pages.resources.unversioned');
}

/** One label when every step loads the same thing, else one per step. */
function normalLabel(resource: LivePageResource, normal: WorkflowUse['normal'], t: Translate): string {
  const labels = normal.map(({ ref }) => laneLabel(resource, ref, t));
  if (new Set(labels).size <= 1) return labels[0] ?? '';
  return normal.map(({ step }, i) => t('pages.resources.stepLabel', step, labels[i])).join(' · ');
}

export function LivePageResources({ pageId, onNavigateWorkflow }: Props) {
  const { t } = useT();
  const [resources, setResources] = useState<LivePageResource[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [promoting, setPromoting] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.resolve().then(() => pagesApi.resources(pageId))
      .then(list => { if (!cancelled) { setResources(list ?? []); setLoadError(null); } })
      .catch(e => { if (!cancelled) setLoadError(String((e as Error).message ?? e)); });
    return () => { cancelled = true; };
  }, [pageId, reload]);

  const refresh = useCallback(() => setReload(n => n + 1), []);

  const promote = useAsyncGuard(async (resource: LivePageResource, use: WorkflowUse, label: string) => {
    const isSkill = resource.kind === 'skill';
    const confirmText = isSkill
      ? t('pages.resources.promoteSkillConfirm', resource.name, label, use.workflowName, use.steps.join(', '))
      : t('pages.resources.promoteQpConfirm', resource.name, label, use.workflowName);
    if (!window.confirm(confirmText)) return;
    setPromoting(`${use.workflowId}:${resource.kind}:${resource.id}`);
    setError(null);
    setDone(null);
    try {
      await workflowsApi.promoteShadow(use.workflowId, { kind: resource.kind, id: resource.id });
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
  const shadowed = resources.filter(r => r.uses.some(use => use.shadow)).length;
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
              {byWorkflow(resource.uses).map(use => (
                <UseRow
                  key={use.workflowId}
                  resource={resource}
                  use={use}
                  busy={promoting === `${use.workflowId}:${resource.kind}:${resource.id}`}
                  onPromote={label => { void promote(resource, use, label); }}
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
          <AppLink to={automationPath({ tab: 'workflows', resourceId: firstWorkflow.workflow_id, editor: 'edit' })}>
            {t('pages.resources.shadowHintLink', firstWorkflow.workflow_name)}
          </AppLink>
        </p>
      )}
    </details>
  );
}

function UseRow({ resource, use, busy, onPromote, onNavigateWorkflow, t }: {
  resource: LivePageResource;
  use: WorkflowUse;
  busy: boolean;
  onPromote: (label: string) => void;
  onNavigateWorkflow?: (workflowId: string) => void;
  t: Translate;
}) {
  const tested = use.shadow ? versionOf(resource, use.shadow) : null;
  const same = tested != null && use.normal.every(({ ref }) => versionOf(resource, ref) === tested);
  const label = tested != null ? `v${tested}` : laneLabel(resource, use.shadow, t, 'shadow');
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
        <span className="live-pages-resource-muted">{t('pages.resources.steps', use.steps.join(', '))}</span>
      </span>
      <span className="live-pages-resource-lanes">
        <span className="live-pages-resource-lane">
          <span className="live-pages-resource-muted">{t('pages.resources.normal')}</span>
          {normalLabel(resource, use.normal, t)}
        </span>
        {use.shadow && (
          <>
            <ArrowRight size={11} aria-hidden className="live-pages-resource-muted" />
            <span className="live-pages-resource-lane" data-lane="shadow">
              <span className="live-pages-resource-muted"><FlaskConical size={10} aria-hidden /> {t('pages.resources.shadow')}</span>
              {laneLabel(resource, use.shadow, t, 'shadow')}
            </span>
          </>
        )}
      </span>
      <span className="live-pages-resource-action">
        {use.shadow && !same && (
          <button
            type="button"
            className="live-pages-resource-promote"
            data-testid={`live-page-resource-promote-${resource.id}`}
            disabled={busy}
            title={t('pages.resources.promoteTitle', use.workflowName)}
            onClick={() => onPromote(label)}
          >
            <ArrowUpCircle size={12} aria-hidden /> {t('pages.resources.promote', label)}
          </button>
        )}
        {use.shadow && same && <span className="live-pages-resource-muted">{t('pages.resources.sameAsNormal')}</span>}
      </span>
    </div>
  );
}

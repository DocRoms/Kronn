// KT-1270 / KT-1336 — the projects a workflow's runs reach beyond their own:
// chosen in one view, validated once by a human, suspended as soon as the
// workflow changes, every project in red behind a typed confirmation.
import { useCallback, useEffect, useMemo, useState } from 'react';
import { CheckCircle2, Globe2, Link2, Search, ShieldAlert } from 'lucide-react';
import { workflows as workflowsApi } from '../lib/api';
import { userError } from '../lib/userError';
import type { ToastFn } from '../hooks/useToast';
import type { Project, ScopeAccess, WorkflowScope as WorkflowScopeData } from '../types/generated';
import './WorkflowScope.css';

type T = (key: string, ...args: (string | number)[]) => string;
type Level = 'none' | ScopeAccess;

const ALL_WRITE_CONFIRMATION = 'ALL-PROJECTS-WRITE';

function useWorkflowScope(workflowId: string) {
  const [loaded, setLoaded] = useState<{ id: string; scope: WorkflowScopeData | null } | null>(null);
  const refresh = useCallback(() => {
    Promise.resolve()
      .then(() => workflowsApi.scope(workflowId))
      .then(scope => setLoaded({ id: workflowId, scope: scope ?? null }))
      .catch(() => setLoaded({ id: workflowId, scope: null }));
  }, [workflowId]);
  useEffect(() => { refresh(); }, [refresh]);
  return { scope: loaded?.id === workflowId ? loaded.scope : null, setLoaded };
}

function accessLabel(t: T, access: ScopeAccess) {
  return access === 'write' ? t('disc.scopeWrite') : t('disc.scopeRead');
}

/** On a launch card: what this workflow's run will reach, and whether the
 *  human validation still holds. Nothing for a workflow without a scope. */
export function WorkflowScopeSummary({ workflowId, t }: { workflowId: string; t: T }) {
  const { scope } = useWorkflowScope(workflowId);
  if (!scope || (scope.entries.length === 0 && !scope.all)) return null;
  const parts = scope.entries.map(entry => `${entry.project_name ?? entry.project_id} (${accessLabel(t, entry.access)})`);
  if (scope.all) parts.unshift(`${t('disc.scopeAllProjects')} (${accessLabel(t, scope.all)})`);
  const list = parts.join(', ');
  return (
    <p
      className="wfs-summary"
      data-testid="workflow-scope-summary"
      data-valid={scope.valid}
      data-all={scope.all ?? undefined}
    >
      {scope.valid
        ? <>{scope.all ? <Globe2 size={12} /> : <Link2 size={12} />} {t('wf.scopeOpens', list)}</>
        : <><ShieldAlert size={12} /> {t('wf.scopeSuspended', list)}</>}
    </p>
  );
}

function LevelPicker({
  value, onChange, disabled, label, t,
}: { value: Level; onChange: (level: Level) => void; disabled?: boolean; label: string; t: T }) {
  const options: Array<[Level, string]> = [
    ['none', t('wf.scopeNone')],
    ['read', t('wf.scopeLevelRead')],
    ['write', t('wf.scopeLevelWrite')],
  ];
  return (
    <div className="wfs-level" role="radiogroup" aria-label={label}>
      {options.map(([level, text]) => (
        <button
          key={level}
          type="button"
          role="radio"
          aria-checked={value === level}
          data-level={level}
          disabled={disabled}
          onClick={() => onChange(level)}
        >
          {text}
        </button>
      ))}
    </div>
  );
}

/** On the workflow page: choose the projects, then validate. Validating is
 *  the human's decision, pinned to the workflow as it is now. */
export function WorkflowScopeEditor({
  workflowId, ownProjectId, projects, toast, t,
}: {
  workflowId: string;
  ownProjectId: string | null;
  projects: Project[];
  toast: ToastFn;
  t: T;
}) {
  const { scope, setLoaded } = useWorkflowScope(workflowId);
  const [draft, setDraft] = useState<{ id: string; levels: Record<string, ScopeAccess>; all: Level } | null>(null);
  const [query, setQuery] = useState('');
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);

  const stored = useMemo(() => {
    const levels: Record<string, ScopeAccess> = {};
    for (const entry of scope?.entries ?? []) levels[entry.project_id] = entry.access;
    return { levels, all: (scope?.all ?? 'none') as Level };
  }, [scope]);
  const current = draft?.id === workflowId ? draft : { id: workflowId, ...stored };
  const changed = JSON.stringify({ l: current.levels, a: current.all }) !== JSON.stringify({ l: stored.levels, a: stored.all });
  const supported = scope?.supported ?? true;
  const valid = scope?.valid ?? false;
  const hasScope = (scope?.entries.length ?? 0) > 0 || !!scope?.all;
  const candidates = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return projects
      .filter(project => project.id !== ownProjectId)
      .filter(project => !needle || project.name.toLowerCase().includes(needle))
      // What the scope already opens comes first, then the rest by name.
      .sort((a, b) => Number(b.id in stored.levels) - Number(a.id in stored.levels) || a.name.localeCompare(b.name));
  }, [projects, ownProjectId, query, stored]);
  const chosen = Object.keys(current.levels).length;
  const allWrite = current.all === 'write';
  const allWriteNew = allWrite && stored.all !== 'write';
  const blockedByConfirm = allWriteNew && typed !== ALL_WRITE_CONFIRMATION;

  const setLevel = (projectId: string, level: Level) => {
    const levels = { ...current.levels };
    if (level === 'none') delete levels[projectId];
    else levels[projectId] = level;
    setDraft({ ...current, levels });
  };
  const setAll = (level: Level) => {
    setTyped('');
    setDraft({ ...current, all: level });
  };

  const run = async (action: () => Promise<WorkflowScopeData>, done: string) => {
    setBusy(true);
    try {
      const saved = await action();
      setLoaded({ id: workflowId, scope: saved });
      setDraft(null);
      setTyped('');
      toast(done, 'success');
    } catch (error) {
      toast(userError(error), 'error');
    } finally {
      setBusy(false);
    }
  };
  const validate = () => run(() => workflowsApi.setScope(workflowId, {
    entries: Object.entries(current.levels).map(([project_id, access]) => ({ project_id, access })),
    all: current.all === 'none' ? null : current.all,
    confirm: allWriteNew ? typed : null,
    expected_fingerprint: allWriteNew ? (scope?.fingerprint ?? null) : null,
  }), t('wf.scopeSaved'));
  // Withdrawing only removes: the remaining entries keep their own validation.
  const revoke = (projectId: string | null) => run(
    () => workflowsApi.revokeScope(workflowId, projectId === null ? { project_id: null, all: true } : { project_id: projectId, all: false }),
    t('disc.scopeRevoked'),
  );

  const status = !supported ? 'unsupported' : !hasScope ? 'unset' : valid ? 'valid' : 'suspended';

  return (
    <section className="wfs" data-testid="workflow-scope-editor" data-status={status}>
      <header className="wfs-head">
        <div className="wfs-head-text">
          <span className="wfs-eyebrow">{t('wf.scopeEyebrow')}</span>
          <h3>{t('wf.scopeTitle')}</h3>
        </div>
        <span className="wfs-status" data-status={status}>
          {status === 'valid' && <CheckCircle2 size={13} />}
          {(status === 'suspended' || status === 'unsupported') && <ShieldAlert size={13} />}
          {t(`wf.scopeStatus.${status}`)}
        </span>
      </header>
      <p className="wfs-hint">{supported ? t('wf.scopeHint') : t('wf.scopeUnsupported')}</p>
      {status === 'suspended' && <p className="wfs-note" data-tone="warning">{t('wf.scopeNeedsValidation')}</p>}

      {!supported ? (
        hasScope && (
          <ul className="wfs-kept">
            {scope?.all && (
              <li data-all="true">
                <Globe2 size={13} /> <strong>{t('disc.scopeAllProjects')}</strong>
                <span className="wfs-tag" data-access={scope.all}>{accessLabel(t, scope.all)}</span>
                <button type="button" className="wfs-btn" data-tone="danger" disabled={busy} onClick={() => void revoke(null)}>
                  {t('disc.scopeRevoke')}
                </button>
              </li>
            )}
            {scope?.entries.map(entry => (
              <li key={entry.project_id}>
                <Link2 size={13} /> <strong>{entry.project_name ?? entry.project_id}</strong>
                <span className="wfs-tag" data-access={entry.access}>{accessLabel(t, entry.access)}</span>
                <button type="button" className="wfs-btn" data-tone="danger" disabled={busy} onClick={() => void revoke(entry.project_id)}>
                  {t('disc.scopeRevoke')}
                </button>
              </li>
            ))}
          </ul>
        )
      ) : (
        <>
          <div className="wfs-all" data-level={current.all}>
            <div className="wfs-all-text">
              <span className="wfs-all-title"><Globe2 size={14} /> {t('disc.scopeAllProjects')}</span>
              <span className="wfs-all-hint">{t('wf.scopeAllHint')}</span>
            </div>
            <LevelPicker value={current.all} onChange={setAll} disabled={busy} label={t('disc.scopeAllProjects')} t={t} />
          </div>
          {allWriteNew && (
            <div className="wfs-confirm" role="alert">
              <p>{t('wf.scopeAllWriteWarning')}</p>
              <label>
                {t('disc.scopeAllWriteType', ALL_WRITE_CONFIRMATION)}
                <input value={typed} onChange={event => setTyped(event.target.value)} spellCheck={false} />
              </label>
            </div>
          )}

          <div className="wfs-toolbar">
            <label className="wfs-search">
              <Search size={14} aria-hidden="true" />
              <input
                type="search"
                value={query}
                onChange={event => setQuery(event.target.value)}
                placeholder={t('wf.scopeSearch')}
                aria-label={t('wf.scopeSearch')}
              />
            </label>
            <span className="wfs-count">{t('wf.scopeChosen', chosen)}</span>
          </div>
          <ul className="wfs-grid">
            {candidates.map(project => {
              const level: Level = current.levels[project.id] ?? 'none';
              return (
                <li key={project.id} className="wfs-project" data-level={level}>
                  <span className="wfs-project-name" title={project.path}>{project.name}</span>
                  <LevelPicker
                    value={level}
                    onChange={next => setLevel(project.id, next)}
                    disabled={busy}
                    label={project.name}
                    t={t}
                  />
                </li>
              );
            })}
            {candidates.length === 0 && <li className="wfs-empty">{t('wf.scopeNoProject')}</li>}
          </ul>

          <footer className="wfs-actions">
            {scope?.validated_by && (
              <span className="wfs-meta">
                {t('wf.scopeValidatedBy', scope.validated_by, scope.validated_at ? new Date(scope.validated_at).toLocaleString() : '?')}
              </span>
            )}
            {changed && (
              <button type="button" className="wfs-btn" disabled={busy} onClick={() => { setDraft(null); setTyped(''); }}>
                {t('disc.scopeCancel')}
              </button>
            )}
            <button
              type="button"
              className="wfs-btn"
              data-tone={allWrite ? 'danger-fill' : 'primary'}
              disabled={busy || blockedByConfirm || (!changed && valid) || (!changed && !hasScope)}
              onClick={() => void validate()}
            >
              {changed || !hasScope ? t('wf.scopeValidate') : valid ? t('wf.scopeUpToDate') : t('wf.scopeRevalidate')}
            </button>
          </footer>
        </>
      )}
    </section>
  );
}

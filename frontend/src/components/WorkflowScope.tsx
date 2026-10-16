// KT-1270 — the projects a workflow's runs reach beyond its own: validated
// once by a human, suspended as soon as the workflow changes.
import { useCallback, useEffect, useState } from 'react';
import { Link2, ShieldAlert } from 'lucide-react';
import { workflows as workflowsApi } from '../lib/api';
import { userError } from '../lib/userError';
import type { ToastFn } from '../hooks/useToast';
import type { Project, ScopeAccess, WorkflowScope as WorkflowScopeData } from '../types/generated';
import './DiscussionScope.css';

type T = (key: string, ...args: (string | number)[]) => string;

function useWorkflowScope(workflowId: string) {
  const [loaded, setLoaded] = useState<{ id: string; scope: WorkflowScopeData | null } | null>(null);
  const refresh = useCallback(() => {
    Promise.resolve()
      .then(() => workflowsApi.scope(workflowId))
      .then(scope => setLoaded({ id: workflowId, scope: scope ?? null }))
      .catch(() => setLoaded({ id: workflowId, scope: null }));
  }, [workflowId]);
  useEffect(() => { refresh(); }, [refresh]);
  return { scope: loaded?.id === workflowId ? loaded.scope : null, refresh, setLoaded };
}

function entryLabel(t: T, name: string, access: ScopeAccess) {
  return `${name} (${access === 'write' ? t('disc.scopeWrite') : t('disc.scopeRead')})`;
}

/** On a launch card: what this workflow's run will reach, and whether the
 *  human validation still holds. Nothing for a workflow without a scope. */
export function WorkflowScopeSummary({ workflowId, t }: { workflowId: string; t: T }) {
  const { scope } = useWorkflowScope(workflowId);
  if (!scope || scope.entries.length === 0) return null;
  const list = scope.entries.map(entry => entryLabel(t, entry.project_name ?? entry.project_id, entry.access)).join(', ');
  return (
    <p className="disc-scope-hint" data-testid="workflow-scope-summary" data-valid={scope.valid}>
      {scope.valid
        ? <><Link2 size={11} /> {t('wf.scopeOpens', list)}</>
        : <><ShieldAlert size={11} /> {t('wf.scopeSuspended', list)}</>}
    </p>
  );
}

/** On the workflow page: set and validate the scope. Saving is the human's
 *  validation, pinned to the workflow as it is now. */
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
  const [project, setProject] = useState('');
  const [access, setAccess] = useState<ScopeAccess>('write');
  const [busy, setBusy] = useState(false);
  const entries = scope?.entries ?? [];

  const save = async (next: { project_id: string; access: ScopeAccess }[]) => {
    setBusy(true);
    try {
      const saved = await workflowsApi.setScope(workflowId, { entries: next });
      setLoaded({ id: workflowId, scope: saved });
      toast(t('wf.scopeSaved'), 'success');
      setProject('');
    } catch (error) {
      toast(userError(error), 'error');
    } finally {
      setBusy(false);
    }
  };
  const current = entries.map(entry => ({ project_id: entry.project_id, access: entry.access }));
  const taken = new Set(current.map(entry => entry.project_id));
  const candidates = projects.filter(p => p.id !== ownProjectId && !taken.has(p.id));

  return (
    <div className="wf-info-row" data-testid="workflow-scope-editor">
      <span className="wf-info-label">{t('wf.scopeTitle')}</span>
      <div className="disc-scope">
        <p className="disc-scope-hint">{t('wf.scopeHint')}</p>
        {scope && entries.length > 0 && !scope.valid && (
          <p className="disc-scope-all-title"><ShieldAlert size={11} /> {t('wf.scopeNeedsValidation')}</p>
        )}
        {entries.length > 0 && (
          <ul className="disc-scope-list">
            {entries.map(entry => (
              <li key={entry.project_id} className="disc-scope-row" data-access={entry.access}>
                <span className="disc-scope-row-main">
                  <Link2 size={11} /> <strong>{entry.project_name ?? entry.project_id}</strong>
                  <span className="disc-scope-access">{entry.access === 'write' ? t('disc.scopeWrite') : t('disc.scopeRead')}</span>
                </span>
                <button
                  type="button"
                  className="btn-danger disc-scope-btn"
                  disabled={busy}
                  onClick={() => void save(current.filter(item => item.project_id !== entry.project_id))}
                >
                  {t('disc.scopeRevoke')}
                </button>
              </li>
            ))}
          </ul>
        )}
        {scope?.validated_by && (
          <span className="disc-scope-meta">
            {t('wf.scopeValidatedBy', scope.validated_by, scope.validated_at ? new Date(scope.validated_at).toLocaleString() : '?')}
          </span>
        )}
        <div className="disc-scope-add">
          <select className="disc-popover-select" value={project} onChange={event => setProject(event.target.value)} aria-label={t('disc.scopeAddProject')}>
            <option value="">{t('disc.scopeAddProject')}</option>
            {candidates.map(candidate => <option key={candidate.id} value={candidate.id}>{candidate.name}</option>)}
          </select>
          <select className="disc-popover-select" value={access} onChange={event => setAccess(event.target.value as ScopeAccess)} aria-label={t('disc.scopeAccess')}>
            <option value="read">{t('disc.scopeRead')}</option>
            <option value="write">{t('disc.scopeWrite')}</option>
          </select>
          <button
            type="button"
            className="disc-scope-btn"
            data-primary="true"
            disabled={!project || busy}
            onClick={() => void save([...current, { project_id: project, access }])}
          >
            {t('disc.scopeGrant')}
          </button>
          {scope && entries.length > 0 && !scope.valid && (
            <button type="button" className="disc-scope-btn" data-primary="true" disabled={busy} onClick={() => void save(current)}>
              {t('wf.scopeRevalidate')}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

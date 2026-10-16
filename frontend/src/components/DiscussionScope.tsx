// KT-1270 — what a room reaches besides its own project, always in view: a
// chip in the header (red for every project), and the settings section where
// a human grants, revokes and decides the agents' requests.
import { useCallback, useEffect, useState } from 'react';
import { Globe2, Link2, ShieldAlert } from 'lucide-react';
import { discussions as discussionsApi } from '../lib/api';
import { userError } from '../lib/userError';
import type { ToastFn } from '../hooks/useToast';
import type {
  DiscussionProjectGrant, DiscussionScope, DiscussionScopeRequest, Project, ScopeAccess,
} from '../types/generated';
import './DiscussionScope.css';

const SCOPE_CHANGED = 'kronn:discussion-scope-changed';
const POLL_MS = 20_000;
const ALL_WRITE_CONFIRMATION = 'ALL-PROJECTS-WRITE';

type T = (key: string, ...args: (string | number)[]) => string;

function announce(discussionId: string) {
  window.dispatchEvent(new CustomEvent(SCOPE_CHANGED, { detail: discussionId }));
}

/** The room's scope, refreshed on a timer (agents file requests at any time)
 *  and whenever this page changes it. */
function useDiscussionScope(discussionId: string, enabled = true) {
  // Keyed by room: switching rooms never shows the previous room's scope.
  const [loaded, setLoaded] = useState<{ id: string; scope: DiscussionScope | null } | null>(null);
  const refresh = useCallback(() => {
    if (!enabled) return;
    // A failed read, even a synchronous one, shows no chip rather than breaking the header.
    Promise.resolve()
      .then(() => discussionsApi.scope(discussionId))
      .then(scope => setLoaded({ id: discussionId, scope: scope ?? null }))
      .catch(() => setLoaded({ id: discussionId, scope: null }));
  }, [discussionId, enabled]);
  const scope = loaded?.id === discussionId ? loaded.scope : null;
  useEffect(() => {
    if (!enabled) return undefined;
    refresh();
    const timer = window.setInterval(refresh, POLL_MS);
    const onChange = (event: Event) => {
      if ((event as CustomEvent<string>).detail === discussionId) refresh();
    };
    window.addEventListener(SCOPE_CHANGED, onChange);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener(SCOPE_CHANGED, onChange);
    };
  }, [discussionId, enabled, refresh]);
  return { scope, refresh };
}

function projectName(projects: Project[], id: string | null | undefined, fallback?: string | null) {
  if (!id) return fallback ?? '?';
  return projects.find(project => project.id === id)?.name ?? fallback ?? id;
}

function accessLabel(t: T, access: ScopeAccess) {
  return access === 'write' ? t('disc.scopeWrite') : t('disc.scopeRead');
}

/** Header chip: nothing for a room on its own project; otherwise the extra
 *  projects, a red badge for every project, and the requests waiting. */
export function DiscussionScopeChip({
  discussionId, hasProject, onOpen, t,
}: { discussionId: string; hasProject: boolean; onOpen?: () => void; t: T }) {
  const { scope } = useDiscussionScope(discussionId, hasProject);
  if (!scope) return null;
  const all = scope.grants.find(grant => grant.project_id === null);
  const extra = scope.grants.filter(grant => grant.project_id !== null);
  const pending = scope.requests.filter(request => request.status === 'pending').length;
  if (!all && extra.length === 0 && pending === 0) return null;
  const writes = extra.some(grant => grant.access === 'write');
  return (
    <span className="disc-scope-chips" data-testid="disc-scope-chip">
      {all && (
        <button
          type="button"
          className="badge badge-error disc-scope-chip"
          onClick={onOpen}
          title={t('disc.scopeAllTitle')}
        >
          <ShieldAlert size={10} /> {all.access === 'write' ? t('disc.scopeAllWrite') : t('disc.scopeAllRead')}
        </button>
      )}
      {extra.length > 0 && (
        <button
          type="button"
          className="disc-scope-chip"
          data-access={writes ? 'write' : 'read'}
          onClick={onOpen}
          title={extra.map(grant => `${grant.project_name ?? grant.project_id} (${accessLabel(t, grant.access)})`).join(', ')}
        >
          <Link2 size={10} /> {t('disc.scopeExtra', extra.length)}
        </button>
      )}
      {pending > 0 && (
        <button
          type="button"
          className="badge badge-warning disc-scope-chip"
          onClick={onOpen}
          data-testid="disc-scope-pending"
        >
          {t('disc.scopePending', pending)}
        </button>
      )}
    </span>
  );
}

function GrantRow({
  grant, projects, onRevoke, t,
}: { grant: DiscussionProjectGrant; projects: Project[]; onRevoke: () => void; t: T }) {
  const all = grant.project_id === null;
  return (
    <li className="disc-scope-row" data-all={all} data-access={grant.access}>
      <span className="disc-scope-row-main">
        {all ? <Globe2 size={11} /> : <Link2 size={11} />}
        <strong>{all ? t('disc.scopeAllProjects') : projectName(projects, grant.project_id, grant.project_name)}</strong>
        <span className="disc-scope-access">{accessLabel(t, grant.access)}</span>
      </span>
      <span className="disc-scope-meta">
        {t('disc.scopeGrantedBy', grant.granted_by, new Date(grant.granted_at).toLocaleString())}
      </span>
      <button type="button" className="btn-danger disc-scope-btn" onClick={onRevoke}>
        {t('disc.scopeRevoke')}
      </button>
    </li>
  );
}

function RequestRow({
  request, projects, onDecide, t,
}: {
  request: DiscussionScopeRequest;
  projects: Project[];
  onDecide: (access: ScopeAccess | null) => void;
  t: T;
}) {
  return (
    <li className="disc-scope-row" data-pending="true">
      <span className="disc-scope-row-main">
        <strong>{request.requested_by}</strong>
        <span>{t('disc.scopeAsks', accessLabel(t, request.access), projectName(projects, request.project_id, request.project_name))}</span>
      </span>
      <span className="disc-scope-reason">{request.reason}</span>
      <span className="disc-scope-actions">
        <button type="button" className="disc-scope-btn" data-primary="true" onClick={() => onDecide(request.access)}>
          {request.access === 'write' ? t('disc.scopeApproveWrite') : t('disc.scopeApproveRead')}
        </button>
        {request.access === 'write' && (
          <button type="button" className="disc-scope-btn" onClick={() => onDecide('read')}>
            {t('disc.scopeApproveRead')}
          </button>
        )}
        <button type="button" className="disc-scope-btn" onClick={() => onDecide(null)}>
          {t('disc.scopeDecline')}
        </button>
      </span>
    </li>
  );
}

/** Settings section: grants with their revoke, pending requests, the add
 *  form, every project (write behind a second typed confirmation), journal. */
export function DiscussionScopeSection({
  discussionId, primaryProjectId, projects, toast, t,
}: {
  discussionId: string;
  primaryProjectId: string | null;
  projects: Project[];
  toast: ToastFn;
  t: T;
}) {
  const { scope, refresh } = useDiscussionScope(discussionId, !!primaryProjectId);
  const [project, setProject] = useState('');
  const [access, setAccess] = useState<ScopeAccess>('read');
  const [confirmingAllWrite, setConfirmingAllWrite] = useState(false);
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);

  if (!primaryProjectId) {
    return <p className="disc-scope-hint">{t('disc.scopeNeedsProject')}</p>;
  }

  const run = async (action: () => Promise<unknown>, done: string) => {
    setBusy(true);
    try {
      await action();
      toast(done, 'success');
      announce(discussionId);
      refresh();
    } catch (error) {
      toast(userError(error), 'error');
    } finally {
      setBusy(false);
    }
  };

  const grants = scope?.grants ?? [];
  const pending = (scope?.requests ?? []).filter(request => request.status === 'pending');
  const all = grants.find(grant => grant.project_id === null);
  const granted = new Set(grants.map(grant => grant.project_id));
  const candidates = projects.filter(p => p.id !== primaryProjectId && !granted.has(p.id));

  return (
    <div className="disc-scope" data-testid="disc-scope-section">
      <p className="disc-scope-hint">{t('disc.scopeHint')}</p>

      {pending.length > 0 && (
        <ul className="disc-scope-list" aria-label={t('disc.scopeRequests')}>
          {pending.map(request => (
            <RequestRow
              key={request.id}
              request={request}
              projects={projects}
              t={t}
              onDecide={decision => void run(
                () => decision
                  ? discussionsApi.approveScopeRequest(discussionId, request.id, { access: decision, decided_by: null })
                  : discussionsApi.declineScopeRequest(discussionId, request.id, { access: null, decided_by: null }),
                decision ? t('disc.scopeGranted') : t('disc.scopeDeclined'),
              )}
            />
          ))}
        </ul>
      )}

      {grants.length > 0 && (
        <ul className="disc-scope-list" aria-label={t('disc.scopeGrants')}>
          {grants.map(grant => (
            <GrantRow
              key={grant.id}
              grant={grant}
              projects={projects}
              t={t}
              onRevoke={() => void run(
                () => discussionsApi.revokeScope(discussionId, grant.id, { revoked_by: null, reason: null }),
                t('disc.scopeRevoked'),
              )}
            />
          ))}
        </ul>
      )}

      <div className="disc-scope-add">
        <select
          className="disc-popover-select"
          value={project}
          onChange={event => setProject(event.target.value)}
          aria-label={t('disc.scopeAddProject')}
        >
          <option value="">{t('disc.scopeAddProject')}</option>
          {candidates.map(candidate => (
            <option key={candidate.id} value={candidate.id}>{candidate.name}</option>
          ))}
        </select>
        <select
          className="disc-popover-select"
          value={access}
          onChange={event => setAccess(event.target.value as ScopeAccess)}
          aria-label={t('disc.scopeAccess')}
        >
          <option value="read">{t('disc.scopeRead')}</option>
          <option value="write">{t('disc.scopeWrite')}</option>
        </select>
        <button
          type="button"
          className="disc-scope-btn"
          data-primary="true"
          disabled={!project || busy}
          onClick={() => void run(
            () => discussionsApi.grantScope(discussionId, {
              project_id: project, all: false, access, granted_by: null, confirm: null, expected_fingerprint: null,
            }).then(() => setProject('')),
            t('disc.scopeGranted'),
          )}
        >
          {t('disc.scopeGrant')}
        </button>
      </div>

      <div className="disc-scope-all">
        <span className="disc-scope-all-title"><ShieldAlert size={11} /> {t('disc.scopeAllProjects')}</span>
        {all?.access !== 'read' && all?.access !== 'write' && (
          <button
            type="button"
            className="disc-scope-btn"
            disabled={busy}
            onClick={() => void run(
              () => discussionsApi.grantScope(discussionId, {
                project_id: null, all: true, access: 'read', granted_by: null, confirm: null, expected_fingerprint: null,
              }),
              t('disc.scopeGranted'),
            )}
          >
            {t('disc.scopeAllRead')}
          </button>
        )}
        {all?.access !== 'write' && !confirmingAllWrite && (
          <button
            type="button"
            className="btn-danger disc-scope-btn"
            disabled={busy || !scope}
            onClick={() => { setConfirmingAllWrite(true); setTyped(''); }}
          >
            {t('disc.scopeAllWrite')}
          </button>
        )}
      </div>

      {confirmingAllWrite && scope && (
        <div className="disc-scope-confirm" role="alertdialog" aria-label={t('disc.scopeAllWrite')}>
          <p>{t('disc.scopeAllWriteWarning')}</p>
          <p>{t('disc.scopeAllWriteType', ALL_WRITE_CONFIRMATION)}</p>
          <input
            className="disc-popover-select"
            value={typed}
            onChange={event => setTyped(event.target.value)}
            aria-label={t('disc.scopeAllWriteType', ALL_WRITE_CONFIRMATION)}
          />
          <span className="disc-scope-actions">
            <button
              type="button"
              className="btn-danger disc-scope-btn"
              disabled={busy || typed !== ALL_WRITE_CONFIRMATION}
              onClick={() => void run(
                () => discussionsApi.grantScope(discussionId, {
                  project_id: null,
                  all: true,
                  access: 'write',
                  granted_by: null,
                  confirm: typed,
                  expected_fingerprint: scope.fingerprint,
                }).then(() => setConfirmingAllWrite(false)),
                t('disc.scopeGranted'),
              )}
            >
              {t('disc.scopeAllWriteConfirm')}
            </button>
            <button type="button" className="disc-scope-btn" onClick={() => setConfirmingAllWrite(false)}>
              {t('disc.scopeCancel')}
            </button>
          </span>
        </div>
      )}

      {(scope?.revoked.length ?? 0) > 0 && (
        <details className="disc-scope-journal">
          <summary>{t('disc.scopeJournal', scope?.revoked.length ?? 0)}</summary>
          <ul>
            {scope?.revoked.map(grant => (
              <li key={grant.id}>
                {grant.project_id === null ? t('disc.scopeAllProjects') : projectName(projects, grant.project_id, grant.project_name)}
                {' · '}{accessLabel(t, grant.access)}{' · '}
                {t('disc.scopeRevokedBy', grant.revoked_by ?? '?', grant.revoked_at ? new Date(grant.revoked_at).toLocaleString() : '?')}
              </li>
            ))}
          </ul>
        </details>
      )}
    </div>
  );
}

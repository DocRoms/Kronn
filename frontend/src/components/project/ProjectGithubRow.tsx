import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { KeyRound, RefreshCw, ShieldAlert } from 'lucide-react';
import { projects as projectsApi } from '../../lib/api';
import { useT } from '../../lib/I18nContext';
import { useAsyncGuard } from '../../hooks/useAsyncGuard';
import type { ProjectGithubConnection } from '../../types/generated';
import {
  dismissGithubNotice,
  githubScopeSummary,
  githubStateTone,
  isGithubConnected,
  isGithubNoticeDismissed,
} from './githubConnection';
import './ProjectGithubRow.css';

/** The confirmation before agents of a project receive a GitHub token. */
export function GithubConnectDialog({
  connection,
  busy,
  error,
  onConnectGhLogin,
  onConnectToken,
  onCancel,
}: {
  connection: ProjectGithubConnection;
  busy: boolean;
  error: string | null;
  onConnectGhLogin: () => void;
  onConnectToken: (token: string) => void;
  onCancel: () => void;
}) {
  const { t } = useT();
  const titleId = useId();
  const tokenId = useId();
  const [token, setToken] = useState('');
  const firstRef = useRef<HTMLButtonElement | HTMLInputElement | null>(null);
  const scope = connection.scope;
  const broad = !!scope?.verified && scope.broad;

  useEffect(() => {
    firstRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onCancel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onCancel]);

  return (
    <div className="gh-dialog-backdrop" onClick={onCancel}>
      <div
        className="gh-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid="github-connect-dialog"
        onClick={event => event.stopPropagation()}
      >
        <h3 id={titleId}>{t('github.dialog.title')}</h3>
        <p className="gh-dialog-risk" role="note">
          <ShieldAlert size={16} aria-hidden="true" />
          <span>{t('github.dialog.risk')}</span>
        </p>
        <section className="gh-dialog-option">
          <strong>{t('github.dialog.ghScope')}</strong>
          {connection.machine_token_available ? (
            <>
              <span className="gh-dialog-scope" data-testid="github-dialog-scope">
                {githubScopeSummary(scope, t)}
              </span>
              {broad && (
                <p className="gh-dialog-recommend" data-testid="github-broad-warning">
                  {t('github.dialog.broadWarning')}
                </p>
              )}
              <button
                type="button"
                className="gh-btn"
                data-variant={broad ? undefined : 'primary'}
                ref={el => { firstRef.current = el; }}
                disabled={busy}
                onClick={onConnectGhLogin}
              >
                {t('github.dialog.useGh')}
              </button>
            </>
          ) : (
            <span className="gh-dialog-scope">{t('github.dialog.noMachineToken')}</span>
          )}
        </section>
        <section className="gh-dialog-option">
          <label htmlFor={tokenId}>
            <strong>{t('github.dialog.tokenLabel')}</strong>
          </label>
          <small>{t('github.dialog.tokenHelp')}</small>
          <input
            id={tokenId}
            type="password"
            autoComplete="off"
            spellCheck={false}
            value={token}
            ref={el => {
              if (!connection.machine_token_available) firstRef.current = el;
            }}
            onChange={event => setToken(event.target.value)}
          />
          <button
            type="button"
            className="gh-btn"
            data-variant={broad ? 'primary' : undefined}
            disabled={busy || token.trim() === ''}
            onClick={() => onConnectToken(token)}
          >
            {t('github.dialog.useToken')}
          </button>
        </section>
        <p className="gh-limit">{t('github.limit')}</p>
        {error && <p className="gh-error" role="alert">{error}</p>}
        <div className="gh-dialog-actions">
          <button type="button" className="gh-btn" onClick={onCancel} disabled={busy}>
            {t('github.dialog.cancel')}
          </button>
        </div>
      </div>
    </div>
  );
}

function TurnOffDialog({
  busy,
  onConfirm,
  onCancel,
}: {
  busy: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useT();
  const titleId = useId();
  const confirmRef = useRef<HTMLButtonElement | null>(null);
  useEffect(() => {
    confirmRef.current?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onCancel();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onCancel]);
  return (
    <div className="gh-dialog-backdrop" onClick={onCancel}>
      <div
        className="gh-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid="github-turn-off-dialog"
        onClick={event => event.stopPropagation()}
      >
        <h3 id={titleId}>{t('github.turnOffDialog.title')}</h3>
        <p>{t('github.turnOffNote')}</p>
        <div className="gh-dialog-actions">
          <button type="button" className="gh-btn" onClick={onCancel} disabled={busy}>
            {t('github.dialog.cancel')}
          </button>
          <button type="button" className="gh-btn" data-variant="danger" ref={confirmRef} onClick={onConfirm} disabled={busy}>
            {t('github.turnOffDialog.confirm')}
          </button>
        </div>
      </div>
    </div>
  );
}

/** The project's GitHub row (Overview): state, scope, connect / turn off. */
export function ProjectGithubRow({ projectId, enabled }: { projectId: string; enabled: boolean }) {
  const { t } = useT();
  const [connection, setConnection] = useState<ProjectGithubConnection | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [dialog, setDialog] = useState<'connect' | 'turnOff' | null>(null);
  const [noticeDismissed, setNoticeDismissed] = useState(() => isGithubNoticeDismissed(projectId));

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    projectsApi.githubConnection(projectId)
      .then(data => { if (!cancelled) { setConnection(data); setError(null); } })
      .catch(err => { if (!cancelled) setError(String(err instanceof Error ? err.message : err)); });
    return () => { cancelled = true; };
  }, [projectId, enabled]);

  const run = useCallback(async (action: () => Promise<ProjectGithubConnection>) => {
    setBusy(true);
    try {
      setConnection(await action());
      setError(null);
      return true;
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
      return false;
    } finally {
      setBusy(false);
    }
  }, []);

  const openConnect = useAsyncGuard(async () => {
    setDialog('connect');
    // The dialog shows the scope the gh token really has.
    await run(() => projectsApi.refreshGithubScope(projectId));
  });
  const connectGhLogin = useAsyncGuard(async () => {
    if (await run(() => projectsApi.setGithubConnection(projectId, { mode: 'gh_login' }))) setDialog(null);
  });
  const connectToken = useAsyncGuard(async (token: string) => {
    if (await run(() => projectsApi.setGithubConnection(projectId, { mode: 'stored_token', token }))) setDialog(null);
  });
  const turnOff = useAsyncGuard(async () => {
    if (await run(() => projectsApi.setGithubConnection(projectId, { mode: 'not_connected' }))) {
      setDialog(null);
      dismissGithubNotice(projectId);
      setNoticeDismissed(true);
    }
  });
  const refreshScope = useAsyncGuard(async () => {
    await run(() => projectsApi.refreshGithubScope(projectId));
  });
  const keep = () => {
    dismissGithubNotice(projectId);
    setNoticeDismissed(true);
  };

  if (!connection) {
    return (
      <div className="project-overview-repository gh-row" data-testid="project-github-row">
        <span className="gh-row-loading" role="status">{error ? t('github.error', error) : t('github.loading')}</span>
      </div>
    );
  }

  const connected = isGithubConnected(connection.state);
  const showNotice = connected && connection.connected_on_upgrade && !noticeDismissed;

  return (
    <div className="project-overview-repository gh-row" data-testid="project-github-row">
      {showNotice && (
        <section className="gh-notice" role="note" aria-labelledby={`gh-notice-${projectId}`} data-testid="github-upgrade-notice">
          <strong id={`gh-notice-${projectId}`}>{t('github.notice.title')}</strong>
          <p>{t('github.notice.body')}</p>
          <p className="gh-limit">{t('github.turnOffNote')}</p>
          <div className="gh-notice-actions">
            <button type="button" className="gh-btn" onClick={() => setDialog('turnOff')} disabled={busy}>
              {t('github.turnOff')}
            </button>
            <button type="button" className="gh-btn" onClick={keep}>
              {t('github.notice.keep')}
            </button>
          </div>
        </section>
      )}
      <div className="project-overview-repository-head">
        <div className="project-overview-repository-title">
          <span className="project-overview-repository-icon" aria-hidden="true">
            <KeyRound size={17} />
          </span>
          <div>
            <span>{t('github.title')}</span>
            <strong>{t(`github.state.${connection.state}`)}</strong>
          </div>
        </div>
        <div className="project-overview-repository-actions">
          {connected ? (
            <button type="button" className="gh-btn" onClick={() => setDialog('turnOff')} disabled={busy}>
              {t('github.turnOff')}
            </button>
          ) : (
            <button type="button" className="gh-btn" data-variant="primary" onClick={() => void openConnect()} disabled={busy}>
              {t('github.connect')}
            </button>
          )}
        </div>
      </div>
      <p className="gh-meaning">{t(`github.meaning.${connection.state}`)}</p>
      <div className="project-overview-repository-meta">
        <span className="project-overview-git-chip" data-tone={githubStateTone(connection.state)}>
          <i aria-hidden="true" />
          {t(`github.state.${connection.state}`)}
        </span>
        {(connected || connection.scope) && (
          <span className="project-overview-git-chip" data-tone={connection.scope?.verified ? undefined : 'warning'} data-testid="github-scope">
            {githubScopeSummary(connection.scope, t)}
          </span>
        )}
        {connection.scope?.login && (
          <span className="project-overview-git-chip">{t('github.scope.account', connection.scope.login)}</span>
        )}
        {(connected || connection.machine_token_available) && (
          <button
            type="button"
            className="project-overview-language-refresh"
            onClick={() => void refreshScope()}
            disabled={busy}
            aria-label={t('github.scope.refresh')}
            title={t('github.scope.refresh')}
          >
            <RefreshCw size={11} className={busy ? 'is-spinning' : undefined} />
          </button>
        )}
      </div>
      {connected && <p className="gh-limit">{t('github.turnOffNote')}</p>}
      <p className="gh-limit">{t('github.limit')}</p>
      {error && !dialog && <p className="gh-error" role="alert">{t('github.error', error)}</p>}
      {dialog === 'connect' && (
        <GithubConnectDialog
          connection={connection}
          busy={busy}
          error={error}
          onConnectGhLogin={() => void connectGhLogin()}
          onConnectToken={token => void connectToken(token)}
          onCancel={() => { setDialog(null); setError(null); }}
        />
      )}
      {dialog === 'turnOff' && (
        <TurnOffDialog busy={busy} onConfirm={() => void turnOff()} onCancel={() => setDialog(null)} />
      )}
    </div>
  );
}

/** The project's GitHub state as a chip, for the discussion header. */
export function GithubConnectionChip({ projectId }: { projectId: string }) {
  const { t } = useT();
  const [connection, setConnection] = useState<ProjectGithubConnection | null>(null);
  useEffect(() => {
    let cancelled = false;
    projectsApi.githubConnection(projectId)
      .then(data => { if (!cancelled) setConnection(data); })
      .catch(() => { if (!cancelled) setConnection(null); });
    return () => { cancelled = true; };
  }, [projectId]);
  if (!connection) return null;
  const label = t(`github.state.${connection.state}`);
  return (
    <span
      className="gh-chip"
      data-tone={githubStateTone(connection.state)}
      data-testid="github-connection-chip"
      title={`${t('github.chip.label', label)} — ${githubScopeSummary(connection.scope, t)}`}
    >
      <KeyRound size={9} aria-hidden="true" />
      {t('github.chip.label', label)}
    </span>
  );
}

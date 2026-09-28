import { useEffect, useState } from 'react';
import { KeyRound, Loader2, ShieldCheck } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatResourceDate } from '../lib/formatResourceDate';
import { loadExecutionSummary, type ExecutionSummary } from '../lib/repositoryResourceExecution';
import type { ResourceRow } from '../lib/repositoryResourceRows';
import { RepositoryResourceModal } from './RepositoryResourceModal';

interface Props {
  row: ResourceRow;
  busy: boolean;
  onApprove: () => void;
  onReject: () => void;
  onAddKey?: () => void;
  onClose: () => void;
}

type SummaryState = { status: 'loading' } | { status: 'ready'; summary: ExecutionSummary | null } | { status: 'failed' };

/** One version, one approval: what will run, which keys it needs, and the way out. */
export function RepositoryResourceApprove({ row, busy, onApprove, onReject, onAddKey, onClose }: Props) {
  const { t, locale } = useT();
  const [state, setState] = useState<SummaryState>({ status: 'loading' });
  const [confirmingReject, setConfirmingReject] = useState(false);

  useEffect(() => {
    let active = true;
    loadExecutionSummary(row.kind, row.id)
      .then(summary => { if (active) setState({ status: 'ready', summary }); })
      .catch(() => { if (active) setState({ status: 'failed' }); });
    return () => { active = false; };
  }, [row.kind, row.id]);

  const fieldValues = (values: string[], field: string) => {
    if (values.length === 0) return <span className="rr-muted">{t('projects.repositoryResources.execution.none')}</span>;
    if (field === 'folder') return <span>{t(`projects.repositoryResources.execution.folder.${values[0]}`)}</span>;
    if (field === 'prompt') return <pre className="rr-code">{values[0]}</pre>;
    return <ul>{values.map((value, index) => <li key={`${value}:${index}`}><code>{value}</code></li>)}</ul>;
  };

  return (
    <RepositoryResourceModal
      testId="repository-approve"
      title={t('projects.repositoryResources.action.approve')}
      subtitle={row.name}
      onClose={onClose}
      footer={<>
        <button type="button" className="rr-button" data-tone="danger" disabled={busy} onClick={() => setConfirmingReject(true)}>
          {t('projects.repositoryResources.approve.reject')}
        </button>
        <button type="button" className="rr-button" data-tone="primary" disabled={busy} onClick={onApprove}>
          {busy ? <Loader2 size={14} className="animate-spin" aria-hidden="true" /> : <ShieldCheck size={14} aria-hidden="true" />}
          {t('projects.repositoryResources.approve.confirm')}
        </button>
      </>}
    >
      <p>{t('projects.repositoryResources.approve.intro')}</p>
      <p className="rr-muted">{t('projects.repositoryResources.approve.oneAtATime')}</p>

      {confirmingReject && (
        <div className="rr-merge" role="alertdialog" aria-label={t('projects.repositoryResources.approve.reject')}>
          <p>{t('projects.repositoryResources.approve.rejectConfirm', row.name)}</p>
          <div className="rr-inline-actions">
            <button type="button" className="rr-button" onClick={() => setConfirmingReject(false)}>{t('common.cancel')}</button>
            <button type="button" className="rr-button" data-tone="danger" disabled={busy} onClick={onReject}>
              {t('projects.repositoryResources.approve.rejectAction')}
            </button>
          </div>
        </div>
      )}

      <section className="rr-section">
        <h3>{t('projects.repositoryResources.approve.version')}</h3>
        <dl className="rr-facts">
          <dt>{t('projects.repositoryResources.compare.path')}</dt>
          <dd><code>{row.displayPath || '—'}</code></dd>
          <dt>{t('projects.repositoryResources.compare.date')}</dt>
          <dd>{formatResourceDate(row.repositoryUpdatedAt, locale)}</dd>
          <dt>{t('projects.repositoryResources.compare.author')}</dt>
          <dd>{row.repositoryUpdatedBy ?? '—'}</dd>
        </dl>
      </section>

      <section className="rr-section">
        <h3>{t('projects.repositoryResources.approve.willRun')}</h3>
        {state.status === 'loading' && (
          <p className="rr-muted"><Loader2 size={14} className="animate-spin" aria-hidden="true" /> {t('projects.repositoryResources.loading')}</p>
        )}
        {state.status !== 'loading' && (state.status === 'failed' || !state.summary) && (
          <p className="rr-muted">{t('projects.repositoryResources.approve.unavailable')}</p>
        )}
        {state.status === 'ready' && state.summary && (
          <dl className="rr-facts">
            {state.summary.fields.map(({ field, values }) => (
              <div key={field} data-field={field}>
                <dt>{t(`projects.repositoryResources.execution.${field}`)}</dt>
                <dd>{fieldValues(values, field)}</dd>
              </div>
            ))}
          </dl>
        )}
      </section>

      <section className="rr-section">
        <h3>{t('projects.repositoryResources.approve.keys')}</h3>
        {row.requiredSecrets.length === 0 ? (
          <p className="rr-muted">{t('projects.repositoryResources.approve.noKeys')}</p>
        ) : (
          <ul className="rr-keys">
            {row.requiredSecrets.map(secret => (
              <li key={secret.name} data-configured={secret.configured}>
                <KeyRound size={13} aria-hidden="true" />
                <code>{secret.name}</code>
                <span>{t(secret.configured
                  ? 'projects.repositoryResources.approve.keyConfigured'
                  : 'projects.repositoryResources.approve.keyMissing')}</span>
                {!secret.configured && onAddKey && (
                  <button type="button" className="rr-button" onClick={onAddKey}>
                    {t('projects.repositoryResources.approve.addKey')}
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>
    </RepositoryResourceModal>
  );
}

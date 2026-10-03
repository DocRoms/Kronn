import { useMemo, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatResourceDate } from '../lib/formatResourceDate';
import { formatFieldValue, parseUnifiedDiff, sideBySide } from '../lib/repositoryResourceDiff';
import type { ResourceRow } from '../lib/repositoryResourceRows';
import { RepositoryResourceModal } from './RepositoryResourceModal';

type DiffMode = 'unified' | 'side';

const DIFF_MARK = { added: '+', removed: '-', context: ' ', hunk: '' } as const;

function DiffView({ diff, mode }: { diff: string; mode: DiffMode }) {
  const lines = useMemo(() => parseUnifiedDiff(diff), [diff]);
  const rows = useMemo(() => sideBySide(lines), [lines]);
  if (mode === 'unified') {
    return (
      <pre className="rr-diff" data-mode="unified">
        {lines.map((line, index) => (
          <span key={index} data-line={line.kind}>
            {line.kind === 'hunk' ? line.text : `${DIFF_MARK[line.kind]}${line.text}`}{'\n'}
          </span>
        ))}
      </pre>
    );
  }
  return (
    <div className="rr-diff" data-mode="side">
      {rows.map((row, index) => (
        <div key={index} className="rr-diff-row" data-line={row.kind}>
          <code data-side="repository" data-empty={row.left === null || undefined}>{row.left ?? ''}</code>
          <code data-side="kronn" data-empty={row.right === null || undefined}>{row.right ?? ''}</code>
        </div>
      ))}
    </div>
  );
}

interface Props {
  row: ResourceRow;
  canWrite: boolean;
  busy: boolean;
  onKeepRepository: () => void;
  onKeepKronn: () => void;
  onRefresh: () => void;
  onPrimary: () => void;
  onClose: () => void;
}

/** Both sides of one resource: where each lives, what differs, and — when the
 *  two versions disagree — the three ways out, each naming what it overwrites. */
export function RepositoryResourceCompare({
  row, canWrite, busy, onKeepRepository, onKeepKronn, onRefresh, onPrimary, onClose,
}: Props) {
  const { t, locale } = useT();
  const [mode, setMode] = useState<DiffMode>('unified');
  const [merging, setMerging] = useState(false);
  const isConflict = row.state === 'conflict';
  const date = (iso?: string) => formatResourceDate(iso, locale);
  const diffs = row.fileDiffs.length > 0
    ? row.fileDiffs
    : row.diff ? [{ path: row.displayPath, diff: row.diff }] : [];
  const repositoryPath = row.displayPath || row.targetPath;
  const primaryLabel = row.primary === 'view' || row.primary === 'compare' || row.primary === 'approve'
    ? null
    : t(`projects.repositoryResources.action.${row.primary}`);

  return (
    <RepositoryResourceModal
      testId="repository-compare"
      title={isConflict ? t('projects.repositoryResources.action.compare') : row.name}
      subtitle={isConflict ? row.name : t(`projects.repositoryResources.status.${row.state}`)}
      onClose={onClose}
      footer={primaryLabel ? (
        <button
          type="button"
          className="rr-button"
          data-tone="primary"
          disabled={busy || (row.primary === 'update_repository' && !canWrite)}
          onClick={onPrimary}
        >
          {primaryLabel}
        </button>
      ) : undefined}
    >
      <div className="rr-sides">
        <section aria-label={t('projects.repositoryResources.columns.repository')}>
          <h3>{t('projects.repositoryResources.columns.repository')}</h3>
          <dl>
            <dt>{t('projects.repositoryResources.compare.path')}</dt>
            <dd><code>{repositoryPath || '—'}</code></dd>
            <dt>{t('projects.repositoryResources.compare.date')}</dt>
            <dd>{date(row.repositoryUpdatedAt)}</dd>
            <dt>{t('projects.repositoryResources.compare.author')}</dt>
            <dd>{row.repositoryUpdatedBy ?? '—'}</dd>
            <dt>{t('projects.repositoryResources.compare.fingerprint')}</dt>
            <dd><code data-testid="fingerprint-repository">{row.repositoryFingerprint ?? '—'}</code></dd>
          </dl>
        </section>
        <section aria-label={t('projects.repositoryResources.columns.kronn')}>
          <h3>{t('projects.repositoryResources.columns.kronn')}</h3>
          <dl>
            <dt>{t('projects.repositoryResources.compare.type')}</dt>
            <dd>
              {t(`projects.repositoryResources.kind.${row.kind}`)}
              {row.level && ` · ${t(`projects.repositoryResources.level.${row.level}`)}`}
            </dd>
            <dt>{t('projects.repositoryResources.compare.scope')}</dt>
            <dd>{t(`projects.repositoryResources.scope.${row.scope}`)}</dd>
            <dt>{t('projects.repositoryResources.compare.date')}</dt>
            <dd>{date(row.kronnUpdatedAt)}</dd>
            <dt>{t('projects.repositoryResources.compare.lastAligned')}</dt>
            <dd>{date(row.alignedAt)}</dd>
            <dt>{t('projects.repositoryResources.compare.fingerprint')}</dt>
            <dd><code data-testid="fingerprint-kronn">{row.kronnFingerprint ?? '—'}</code></dd>
          </dl>
        </section>
      </div>

      {row.paths.length > 1 && (
        <section className="rr-section">
          <h3>{t('projects.repositoryResources.detail.copies')}</h3>
          <ul className="rr-path-list">
            {row.paths.map(path => <li key={path}><code>{path}</code></li>)}
          </ul>
        </section>
      )}

      {(row.fieldDiff.length > 0 || diffs.length > 0) && (
        <section className="rr-section">
          <h3>{t('projects.repositoryResources.compare.differs')}</h3>
          {row.fieldDiff.length > 0 && (
            <table className="rr-fields">
              <thead>
                <tr>
                  <th scope="col">{t('projects.repositoryResources.compare.field')}</th>
                  <th scope="col">{t('projects.repositoryResources.columns.repository')}</th>
                  <th scope="col">{t('projects.repositoryResources.columns.kronn')}</th>
                </tr>
              </thead>
              <tbody>
                {row.fieldDiff.map(field => (
                  <tr key={field.field}>
                    <th scope="row"><code>{field.field}</code></th>
                    <td><code>{formatFieldValue(field.repository)}</code></td>
                    <td><code>{formatFieldValue(field.kronn)}</code></td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {diffs.length > 0 && (
            <>
              <div className="rr-diff-toolbar">
                <span>{t('projects.repositoryResources.compare.files', diffs.length)}</span>
                <div role="group" aria-label={t('projects.repositoryResources.compare.view')}>
                  {(['unified', 'side'] as const).map(option => (
                    <button
                      key={option}
                      type="button"
                      className="rr-chip"
                      aria-pressed={mode === option}
                      onClick={() => setMode(option)}
                    >
                      {t(`projects.repositoryResources.compare.${option}`)}
                    </button>
                  ))}
                </div>
              </div>
              {diffs.map(file => (
                <div key={file.path} className="rr-diff-file">
                  <code className="rr-diff-path">{file.path}</code>
                  <DiffView diff={file.diff} mode={mode} />
                </div>
              ))}
            </>
          )}
        </section>
      )}
      {row.fieldDiff.length === 0 && diffs.length === 0 && (
        <p className="rr-muted">{t('projects.repositoryResources.compare.noDiff')}</p>
      )}

      {row.writePreview.length > 0 && !isConflict && (
        <section className="rr-section">
          <h3>{t('projects.repositoryResources.detail.writes')}</h3>
          <ul className="rr-path-list">
            {row.writePreview.map(path => <li key={path}><code>{path}</code></li>)}
          </ul>
        </section>
      )}

      {isConflict && (
        <section className="rr-section">
          <h3>{t('projects.repositoryResources.compare.choices')}</h3>
          <div className="rr-choices">
            <article data-choice="repository">
              <h4>{t('projects.repositoryResources.compare.keepRepository')}</h4>
              <p>{t('projects.repositoryResources.compare.keepRepositoryEffect', date(row.kronnUpdatedAt))}</p>
              <button type="button" className="rr-button" disabled={busy} onClick={onKeepRepository}>
                {busy && <Loader2 size={14} className="animate-spin" aria-hidden="true" />}
                {t('projects.repositoryResources.compare.keepRepository')}
              </button>
            </article>
            <article data-choice="kronn">
              <h4>{t('projects.repositoryResources.compare.keepKronn')}</h4>
              <p>{t('projects.repositoryResources.compare.keepKronnEffect', repositoryPath, date(row.repositoryUpdatedAt))}</p>
              <button type="button" className="rr-button" disabled={busy || !canWrite} onClick={onKeepKronn}>
                {busy && <Loader2 size={14} className="animate-spin" aria-hidden="true" />}
                {t('projects.repositoryResources.compare.keepKronn')}
              </button>
              {!canWrite && <small>{t('projects.repositoryResources.banner.writeDisabled.title')}</small>}
            </article>
            <article data-choice="merge">
              <h4>{t('projects.repositoryResources.compare.merge')}</h4>
              <p>{t('projects.repositoryResources.compare.mergeEffect')}</p>
              <button type="button" className="rr-button" aria-expanded={merging} onClick={() => setMerging(open => !open)}>
                {t('projects.repositoryResources.compare.merge')}
              </button>
              {merging && (
                <div className="rr-merge" role="region" aria-label={t('projects.repositoryResources.compare.merge')}>
                  <p>{t('projects.repositoryResources.compare.mergeSteps', repositoryPath)}</p>
                  <button type="button" className="rr-button" onClick={onRefresh}>
                    {t('projects.repositoryResources.compare.refresh')}
                  </button>
                </div>
              )}
            </article>
          </div>
        </section>
      )}
    </RepositoryResourceModal>
  );
}

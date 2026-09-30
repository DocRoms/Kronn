import { useEffect, useState } from 'react';
import { Loader2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { formatResourceDate } from '../lib/formatResourceDate';
import { formatFieldValue } from '../lib/repositoryResourceDiff';
import type { ResourceRow, RowLink } from '../lib/repositoryResourceRows';
import type { RepositoryResourceComparison } from '../types/generated';
import { RepositoryResourceContent } from './RepositoryResourceContent';
import { RepositoryResourceModal } from './RepositoryResourceModal';

type ComparisonPhase = 'loading' | 'ready' | 'error';

/** What sits behind a listed row: the text of each side and the diffs. The
 *  listing only says that the two sides differ and carries no content; the
 *  backend builds it when this sheet opens, so nothing is read or masked for the
 *  rows nobody opens. Every row has at least one side to show. */
function useComparison(projectId: string, row: ResourceRow) {
  const [attempt, setAttempt] = useState(0);
  // The answer is filed under the request it belongs to, so a stale answer, or a
  // row whose content changed underneath the open sheet, reads as "loading"
  // again instead of showing the wrong diff.
  const requestKey = [projectId, row.kind, row.id, row.state, row.repositoryFingerprint, row.kronnFingerprint, attempt].join('|');
  const [settled, setSettled] = useState<{ key: string; comparison?: RepositoryResourceComparison } | null>(null);

  useEffect(() => {
    let active = true;
    projectsApi.repositoryResourceComparison(projectId, row.kind, row.id)
      .then(comparison => { if (active) setSettled({ key: requestKey, comparison }); })
      .catch(() => { if (active) setSettled({ key: requestKey }); });
    return () => { active = false; };
  }, [projectId, row.kind, row.id, requestKey]);

  let phase: ComparisonPhase = 'loading';
  if (settled?.key === requestKey) phase = settled.comparison ? 'ready' : 'error';
  return {
    phase,
    comparison: phase === 'ready' ? settled?.comparison : undefined,
    retry: () => setAttempt(count => count + 1),
  };
}

/** One side of the reference graph: a link opens the resource's own sheet,
 *  unless nothing answers to it (a missing reference). */
function LinkList({ title, links, openable, onOpen }: {
  title: string;
  links: RowLink[];
  openable: ReadonlySet<string>;
  onOpen: (key: string) => void;
}) {
  const { t } = useT();
  return (
    <div>
      <h3>{title} <span>{links.length}</span></h3>
      {links.length === 0 ? (
        <p className="rr-muted">{t('projects.repositoryResources.links.none')}</p>
      ) : (
        <ul className="rr-path-list">
          {links.map(link => (
            <li key={link.key} data-missing={link.missing || undefined}>
              {!link.missing && openable.has(link.key) ? (
                <button type="button" className="rr-link" onClick={() => onOpen(link.key)}>{link.name}</button>
              ) : (
                <span>{link.name}</span>
              )}
              {' '}<small>{t(`projects.repositoryResources.kind.${link.kind}`)}</small>
              {link.missing && (
                <span className="rr-pill" data-state="missing" title={t('projects.repositoryResources.links.missingHint')}>
                  {t('projects.repositoryResources.links.missing')}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

interface Props {
  projectId: string;
  row: ResourceRow;
  /** Keys of the rows a link can open. */
  openable: ReadonlySet<string>;
  canWrite: boolean;
  busy: boolean;
  onKeepRepository: () => void;
  onKeepKronn: () => void;
  onRefresh: () => void;
  onPrimary: () => void;
  onOpenLink: (key: string) => void;
  onClose: () => void;
}

/** Both sides of one resource: where each lives, what each holds (the file as
 *  the repository has it, as Kronn has it, or what differs) and — when the two
 *  versions disagree — the three ways out, each naming what it overwrites. */
export function RepositoryResourceCompare({
  projectId, row, openable, canWrite, busy, onKeepRepository, onKeepKronn, onRefresh, onPrimary, onOpenLink, onClose,
}: Props) {
  const { t, locale } = useT();
  const [merging, setMerging] = useState(false);
  const isConflict = row.state === 'conflict';
  const date = (iso?: string) => formatResourceDate(iso, locale);
  const { phase, comparison, retry } = useComparison(projectId, row);
  const fieldDiff = comparison?.field_diff ?? [];
  const fileDiffs = comparison?.file_diffs ?? [];
  const diffs = fileDiffs.length > 0
    ? fileDiffs
    : comparison?.diff ? [{ path: row.displayPath, diff: comparison.diff }] : [];
  // Nobody should overwrite a side before the differences are on screen.
  const reviewing = phase === 'loading';
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

      {row.kind !== 'skill' && (
        <section className="rr-section rr-link-lists" data-testid="resource-links">
          <LinkList title={t('projects.repositoryResources.links.uses')} links={row.uses} openable={openable} onOpen={onOpenLink} />
          <LinkList title={t('projects.repositoryResources.links.usedBy')} links={row.usedBy} openable={openable} onOpen={onOpenLink} />
        </section>
      )}

      {row.paths.length > 1 && (
        <section className="rr-section">
          <h3>{t('projects.repositoryResources.detail.copies')}</h3>
          <ul className="rr-path-list">
            {row.paths.map(path => <li key={path}><code>{path}</code></li>)}
          </ul>
        </section>
      )}

      {phase === 'loading' && (
        <section className="rr-section" aria-busy="true" data-testid="repository-compare-loading">
          <h3>{t('projects.repositoryResources.content.title')}</h3>
          <p className="rr-muted" role="status">
            <Loader2 size={14} className="animate-spin" aria-hidden="true" />{' '}
            {t('projects.repositoryResources.compare.loading')}
          </p>
        </section>
      )}
      {phase === 'error' && (
        <section className="rr-section">
          <h3>{t('projects.repositoryResources.content.title')}</h3>
          <p className="rr-muted" role="alert">{t('projects.repositoryResources.compare.loadFailed')}</p>
          <button type="button" className="rr-button" onClick={retry}>
            {t('projects.repositoryResources.compare.retry')}
          </button>
        </section>
      )}
      {fieldDiff.length > 0 && (
        <section className="rr-section">
          <h3>{t('projects.repositoryResources.compare.differs')}</h3>
          <table className="rr-fields">
            <thead>
              <tr>
                <th scope="col">{t('projects.repositoryResources.compare.field')}</th>
                <th scope="col">{t('projects.repositoryResources.columns.repository')}</th>
                <th scope="col">{t('projects.repositoryResources.columns.kronn')}</th>
              </tr>
            </thead>
            <tbody>
              {fieldDiff.map(field => (
                <tr key={field.field}>
                  <th scope="row"><code>{field.field}</code></th>
                  <td><code>{formatFieldValue(field.repository)}</code></td>
                  <td><code>{formatFieldValue(field.kronn)}</code></td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      )}
      {comparison && <RepositoryResourceContent row={row} comparison={comparison} diffs={diffs} />}

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
              <button type="button" className="rr-button" disabled={busy || reviewing} onClick={onKeepRepository}>
                {busy && <Loader2 size={14} className="animate-spin" aria-hidden="true" />}
                {t('projects.repositoryResources.compare.keepRepository')}
              </button>
            </article>
            <article data-choice="kronn">
              <h4>{t('projects.repositoryResources.compare.keepKronn')}</h4>
              <p>{t('projects.repositoryResources.compare.keepKronnEffect', repositoryPath, date(row.repositoryUpdatedAt))}</p>
              <button type="button" className="rr-button" disabled={busy || reviewing || !canWrite} onClick={onKeepKronn}>
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

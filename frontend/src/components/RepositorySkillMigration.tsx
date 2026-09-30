import { useEffect, useState } from 'react';
import { AlertTriangle, ArrowRight, GitBranch, Loader2 } from 'lucide-react';
import { projects as projectsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import {
  migrationSize,
  resolutionsOf,
  SKILLS_TARGET_ROOT,
  unresolvedConflicts,
  type MigrationChoices,
} from '../lib/skillMigration';
import { userError } from '../lib/userError';
import type { SkillMigrationPlan, SkillMigrationResult } from '../types/generated';
import { RepositoryResourceModal } from './RepositoryResourceModal';

interface Props {
  projectId: string;
  canWrite: boolean;
  /** `migrated` is true once files were moved: the listing must be read again. */
  onClose: (migrated: boolean) => void;
  onOpenGit?: () => void;
}

/** "Migrate everything to .agents/skills": the recap of every move (source →
 *  target) and every conflict comes first, nothing is written before the
 *  confirmation, a conflict without a chosen version is skipped, and nothing
 *  is committed. */
export function RepositorySkillMigration({ projectId, canWrite, onClose, onOpenGit }: Props) {
  const { t } = useT();
  const [plan, setPlan] = useState<SkillMigrationPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [choices, setChoices] = useState<MigrationChoices>({});
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<SkillMigrationResult | null>(null);

  useEffect(() => {
    let active = true;
    projectsApi.skillMigrationPlan(projectId).then(loaded => {
      if (active) setPlan(loaded);
    }).catch(reason => {
      if (active) setError(userError(reason));
    });
    return () => { active = false; };
  }, [projectId]);

  const migrate = async () => {
    if (!plan) return;
    setBusy(true);
    setError(null);
    try {
      setResult(await projectsApi.migrateSkills(projectId, { resolutions: resolutionsOf(plan, choices) }));
    } catch (reason) {
      setError(userError(reason));
    } finally {
      setBusy(false);
    }
  };

  const size = plan ? migrationSize(plan, choices) : 0;
  const skipped = plan ? unresolvedConflicts(plan, choices) : 0;
  const empty = plan !== null && plan.moves.length === 0 && plan.conflicts.length === 0;

  return (
    <RepositoryResourceModal
      size="dialog"
      testId="skill-migration"
      title={t('projects.repositoryResources.skillMigration.title', SKILLS_TARGET_ROOT)}
      onClose={() => onClose(result !== null)}
      footer={result ? (
        <button type="button" className="rr-button" data-tone="primary" onClick={() => onClose(true)}>
          {t('common.close')}
        </button>
      ) : <>
        <button type="button" className="rr-button" onClick={() => onClose(false)}>{t('common.cancel')}</button>
        <button
          type="button"
          className="rr-button"
          data-tone="primary"
          disabled={busy || !canWrite || size === 0}
          title={canWrite ? undefined : t('projects.repositoryResources.banner.writeDisabled.title')}
          onClick={() => { void migrate(); }}
        >
          {busy && <Loader2 size={14} className="animate-spin" aria-hidden="true" />}
          {t('projects.repositoryResources.skillMigration.confirm', size)}
        </button>
      </>}
    >
      {error && (
        <p className="rr-link-note" data-tone="error" role="alert">
          <AlertTriangle size={14} aria-hidden="true" /> {error}
        </p>
      )}
      {!plan && !error && (
        <p className="rr-muted"><Loader2 size={14} className="animate-spin" aria-hidden="true" /> {t('projects.repositoryResources.skillMigration.loading')}</p>
      )}
      {result && (
        <section className="rr-section" data-testid="skill-migration-done">
          <h3>{t('projects.repositoryResources.skillMigration.done.title')}</h3>
          <p>{t('projects.repositoryResources.skillMigration.done.moved', result.moved.length)}</p>
          {result.unresolved.length > 0 && (
            <p className="rr-muted">{t('projects.repositoryResources.skillMigration.done.unresolved', result.unresolved.length)}</p>
          )}
          {result.kept.length > 0 && (
            <p className="rr-muted">{t('projects.repositoryResources.skillMigration.done.kept', result.kept.length)}</p>
          )}
          <p className="rr-muted">
            <GitBranch size={13} aria-hidden="true" /> {t('projects.repositoryResources.skillMigration.done.notCommitted')}
          </p>
          {onOpenGit && (
            <button type="button" className="rr-link" onClick={() => { onClose(true); onOpenGit(); }}>
              {t('projects.repositoryResources.banner.openGit')}
            </button>
          )}
        </section>
      )}
      {plan && !result && (
        <>
          <p className="rr-muted">{t('projects.repositoryResources.skillMigration.intro', plan.target_root)}</p>
          {empty && plan.blocked.length === 0 && (
            <p>{t('projects.repositoryResources.skillMigration.empty')}</p>
          )}
          {plan.moves.length > 0 && (
            <section className="rr-section" data-testid="skill-migration-moves">
              <h3>{t('projects.repositoryResources.skillMigration.moves', plan.moves.length)}</h3>
              <ul className="rr-migration-list">
                {plan.moves.map(move => (
                  <li key={move.source}>
                    <code>{move.source}/</code>
                    <ArrowRight size={13} aria-hidden="true" />
                    <code>{move.target}/</code>
                    {move.action === 'duplicate' && (
                      <small>{t('projects.repositoryResources.skillMigration.duplicate')}</small>
                    )}
                    {move.converted && (
                      <small>{t('projects.repositoryResources.skillMigration.converted')}</small>
                    )}
                    {move.kronn_managed && (
                      <small>{t('projects.repositoryResources.skillMigration.managed')}</small>
                    )}
                  </li>
                ))}
              </ul>
            </section>
          )}
          {plan.conflicts.length > 0 && (
            <section className="rr-section" data-testid="skill-migration-conflicts">
              <h3>{t('projects.repositoryResources.skillMigration.conflicts', plan.conflicts.length)}</h3>
              <p className="rr-muted">{t('projects.repositoryResources.skillMigration.conflictsHint')}</p>
              {plan.conflicts.map(conflict => (
                <fieldset key={conflict.slug} className="rr-migration-conflict" data-slug={conflict.slug}>
                  <legend>
                    <strong>{conflict.slug}</strong> <ArrowRight size={13} aria-hidden="true" /> <code>{conflict.target}/</code>
                  </legend>
                  {conflict.versions.map(version => (
                    <label key={version.fingerprint}>
                      <input
                        type="radio"
                        name={`keep-${conflict.slug}`}
                        checked={version.paths.includes(choices[conflict.slug])}
                        disabled={busy}
                        onChange={() => setChoices(current => ({ ...current, [conflict.slug]: version.paths[0] }))}
                      />
                      <span>
                        {version.paths.map(path => <code key={path}>{path}/</code>)}
                        <small>
                          {version.fingerprint}
                          {version.at_target && ` · ${t('projects.repositoryResources.skillMigration.atTarget', plan.target_root)}`}
                        </small>
                      </span>
                    </label>
                  ))}
                  <label>
                    <input
                      type="radio"
                      name={`keep-${conflict.slug}`}
                      checked={!choices[conflict.slug]}
                      disabled={busy}
                      onChange={() => setChoices(current => Object.fromEntries(
                        Object.entries(current).filter(([slug]) => slug !== conflict.slug),
                      ))}
                    />
                    <span>{t('projects.repositoryResources.skillMigration.skip')}</span>
                  </label>
                </fieldset>
              ))}
              {skipped > 0 && (
                <p className="rr-muted" data-testid="skill-migration-skipped">
                  {t('projects.repositoryResources.skillMigration.skipped', skipped)}
                </p>
              )}
            </section>
          )}
          {plan.blocked.length > 0 && (
            <section className="rr-section" data-testid="skill-migration-blocked">
              <h3>{t('projects.repositoryResources.skillMigration.blocked', plan.blocked.length)}</h3>
              <ul className="rr-migration-list">
                {plan.blocked.map(item => (
                  <li key={item.path}>
                    <code>{item.path}/</code>
                    <small>{t(`projects.repositoryResources.skillMigration.reason.${item.reason}`)}</small>
                  </li>
                ))}
              </ul>
            </section>
          )}
          <p className="rr-muted">
            <GitBranch size={13} aria-hidden="true" /> {t('projects.repositoryResources.skillMigration.noCommit')}
          </p>
        </>
      )}
    </RepositoryResourceModal>
  );
}

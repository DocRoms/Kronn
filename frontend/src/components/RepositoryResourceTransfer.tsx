import { useState } from 'react';
import { GitCommitHorizontal, Loader2, PenLine, Play, ShieldAlert, type LucideIcon } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatResourceDate } from '../lib/formatResourceDate';
import { describeTransfer, type EffectTone, type TransferKind, type TransferPlan } from '../lib/repositoryResourceEffects';
import { RepositoryResourceModal } from './RepositoryResourceModal';

const TONE_ICON: Record<EffectTone, LucideIcon> = {
  write: PenLine,
  commit: GitCommitHorizontal,
  activation: Play,
  loss: ShieldAlert,
};

const transferLabelKey = (kind: TransferKind): string => (
  kind === 'publish_selected'
    ? 'projects.repositoryResources.publishSelected'
    : `projects.repositoryResources.action.${kind}`
);

interface Props {
  plan: TransferPlan;
  kronnExists: boolean;
  busy: boolean;
  onConfirm: (nativePath?: string) => void;
  onCancel: () => void;
}

/** The effect of a transfer, in plain sentences, before anything moves. */
export function RepositoryResourceTransfer({ plan, kronnExists, busy, onConfirm, onCancel }: Props) {
  const { t, locale } = useT();
  const [row] = plan.rows;
  const nativeChoice = plan.kind === 'use_native' || plan.kind === 'copy_native';
  const paths = nativeChoice ? row.paths : [];
  const [chosen, setChosen] = useState(row.pathsDiverge ? '' : paths[0] ?? '');
  const needsChoice = nativeChoice && paths.length > 1 && row.pathsDiverge;
  const effects = describeTransfer(plan, {
    kronnExists,
    nativePath: nativeChoice ? chosen || paths[0] : undefined,
    formatDate: iso => formatResourceDate(iso, locale),
  });
  const label = plan.kind === 'publish_selected'
    ? t(transferLabelKey(plan.kind), plan.rows.length)
    : t(transferLabelKey(plan.kind));

  return (
    <RepositoryResourceModal
      size="dialog"
      testId="repository-transfer"
      title={label}
      subtitle={plan.kind === 'publish_selected' ? t('projects.repositoryResources.transfer.selection', plan.rows.length) : row.name}
      onClose={onCancel}
      footer={<>
        <button type="button" className="rr-button" onClick={onCancel}>{t('common.cancel')}</button>
        <button
          type="button"
          className="rr-button"
          data-tone="primary"
          disabled={busy || (needsChoice && !chosen)}
          onClick={() => onConfirm(nativeChoice ? chosen || paths[0] : undefined)}
        >
          {busy && <Loader2 size={14} className="animate-spin" aria-hidden="true" />}
          {label}
        </button>
      </>}
    >
      {needsChoice && (
        <fieldset className="rr-fieldset">
          <legend>{t('projects.repositoryResources.transfer.pickPath')}</legend>
          <p>{t('projects.repositoryResources.transfer.pickRequired')}</p>
          {paths.map(path => (
            <label key={path}>
              <input type="radio" name="native-path" value={path} checked={chosen === path} onChange={() => setChosen(path)} />
              <code>{path}</code>
            </label>
          ))}
        </fieldset>
      )}
      <ul className="rr-effects" aria-label={t('projects.repositoryResources.transfer.effects')}>
        {effects.map(effect => {
          const Icon = TONE_ICON[effect.tone];
          return (
            <li key={`${effect.tone}:${effect.key}`} data-tone={effect.tone}>
              <Icon size={14} aria-hidden="true" />
              <span>{t(effect.key, ...effect.args)}</span>
            </li>
          );
        })}
      </ul>
    </RepositoryResourceModal>
  );
}

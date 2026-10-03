import { useState } from 'react';
import { GitCommitHorizontal, Loader2, PenLine, Play, ShieldAlert, type LucideIcon } from 'lucide-react';
import { useT } from '../lib/I18nContext';
import { formatResourceDate } from '../lib/formatResourceDate';
import { describeTransfer, transferSide, type EffectTone, type TransferKind, type TransferPlan } from '../lib/repositoryResourceEffects';
import type { TransferLinks } from '../lib/repositoryResourceLinks';
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

export interface TransferChoice {
  nativePath?: string;
  /** Also move the linked items the resource needs (the default). */
  includeLinked: boolean;
}

interface Props {
  plan: TransferPlan;
  /** What the resource brings with it, when it is a one-resource transfer. */
  links?: TransferLinks;
  kronnExists: boolean;
  busy: boolean;
  onConfirm: (choice: TransferChoice) => void;
  onCancel: () => void;
}

/** Announces the resources a transfer depends on and offers to move them too:
 *  included by default, and leaving them out says what stops working. */
function LinkedItems({ name, links, side, include, onInclude }: {
  name: string;
  links: TransferLinks;
  side: 'repository' | 'kronn';
  include: boolean;
  onInclude: (include: boolean) => void;
}) {
  const { t } = useT();
  const { pending, settled, missing } = links;
  if (pending.length + settled.length + missing.length === 0) return null;
  return (
    <section className="rr-section rr-linked" data-testid="transfer-linked">
      <h3>{t('projects.repositoryResources.links.transfer.title')}</h3>
      {pending.length > 0 && (
        <>
          <p>
            {side === 'repository'
              ? t('projects.repositoryResources.links.transfer.introRepository', name, pending.length)
              : t('projects.repositoryResources.links.transfer.introKronn', name, pending.length)}
          </p>
          <ul className="rr-path-list">
            {pending.map(item => (
              <li key={item.key}>
                {item.name} <small>{t(`projects.repositoryResources.kind.${item.kind}`)}</small>
              </li>
            ))}
          </ul>
          <label className="rr-linked-include">
            <input type="checkbox" checked={include} onChange={event => onInclude(event.target.checked)} />
            {t('projects.repositoryResources.links.transfer.include', pending.length)}
          </label>
          {!include && (
            <p className="rr-linked-warning" role="alert">
              {t('projects.repositoryResources.links.transfer.partial', name)}
            </p>
          )}
        </>
      )}
      {settled.length > 0 && (
        <p className="rr-muted">{t('projects.repositoryResources.links.transfer.settled', settled.length)}</p>
      )}
      {missing.length > 0 && (
        <p className="rr-linked-warning">
          {t('projects.repositoryResources.links.transfer.missing', missing.length, missing.map(link => link.name).join(', '))}
        </p>
      )}
    </section>
  );
}

/** The effect of a transfer, in plain sentences, before anything moves. */
export function RepositoryResourceTransfer({ plan, links, kronnExists, busy, onConfirm, onCancel }: Props) {
  const { t, locale } = useT();
  const [row] = plan.rows;
  const nativeChoice = plan.kind === 'use_native' || plan.kind === 'copy_native';
  const paths = nativeChoice ? row.paths : [];
  const [chosen, setChosen] = useState(row.pathsDiverge ? '' : paths[0] ?? '');
  const [includeLinked, setIncludeLinked] = useState(true);
  const side = transferSide(plan.kind);
  const linked = links && includeLinked ? links.pending : [];
  const needsChoice = nativeChoice && paths.length > 1 && row.pathsDiverge;
  const effects = describeTransfer({ ...plan, linked }, {
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
          onClick={() => onConfirm({ nativePath: nativeChoice ? chosen || paths[0] : undefined, includeLinked })}
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
      {links && side && (
        <LinkedItems name={row.name} links={links} side={side} include={includeLinked} onInclude={setIncludeLinked} />
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

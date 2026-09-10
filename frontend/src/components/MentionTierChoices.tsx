import { useId } from 'react';
import { MODEL_TIER_ICONS } from '../lib/constants';
import type { ResolvedCatalogTier } from '../lib/modelCatalogSelection';
import type { ModelTier } from '../types/generated';
import { MENTION_TIER_CHOICES } from '../lib/mentionTierSelection';

export function MentionTierChoices({ trigger, currentTier, keyboardTier, ariaLabel, resolve, onSelect, t }: {
  trigger: string;
  currentTier?: ModelTier;
  keyboardTier?: ModelTier;
  ariaLabel?: string;
  resolve: (tier: ModelTier) => ResolvedCatalogTier;
  onSelect: (tier: ModelTier) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}) {
  const id = useId();
  return <span className="disc-mention-tier-choices" aria-label={ariaLabel}>
    {MENTION_TIER_CHOICES.map(tier => {
      const state = resolve(tier);
      const title = t('disc.routingInvokeTier', t(`disc.tier.${tier}`), state.model || t('disc.defaultAgentModel'));
      const provenance = state.provenance ? t(`modelCatalog.provenance.${state.provenance}`)
        : state.configured ? t('modelCatalog.notInCatalog') : '';
      const descriptionId = `${id}-${tier}`;
      return <button key={tier} type="button" className="disc-mention-tier-choice"
        data-tier={tier} data-current={currentTier === tier} data-keyboard-selected={keyboardTier === tier}
        aria-label={`${trigger} · ${title}`} aria-describedby={descriptionId} title={title}
        disabled={state.unavailable}
        onMouseDown={event => {
          event.preventDefault();
          event.stopPropagation();
          if (!state.unavailable) onSelect(tier);
        }}
        onClick={event => {
          // Keyboard/assistive activation; pointer selection already ran on mousedown.
          if (event.detail === 0 && !state.unavailable) onSelect(tier);
        }}>
        <span aria-hidden="true">{MODEL_TIER_ICONS[tier]}</span>
        <span id={descriptionId} hidden>{[title, state.entry?.model_id, provenance,
          state.unavailable ? t('modelCatalog.unavailable') : '', state.entry?.unavailable_reason,
          state.entry ? t('modelCatalog.lastChecked', state.entry.last_checked_at) : '',
        ].filter(Boolean).join(' · ')}</span>
        {(state.entry || state.configured) && <span className="disc-mention-catalog-meta">
          {state.unavailable ? t('modelCatalog.unavailable') : provenance}
        </span>}
      </button>;
    })}
  </span>;
}

import type { ReactNode } from 'react';
import { Plus } from 'lucide-react';
import { AgentSwitchPicker } from './AgentSwitchPicker';
import type { AgentSwitchTarget } from './AgentSwitchPicker';
import { AGENT_LABELS, MODEL_TIER_ICONS, agentColor } from '../lib/constants';
import type { UnusableExternalAgentTarget } from '../lib/externalAgentIdentity';
import { useT } from '../lib/I18nContext';
import type { AgentType, ModelTier, ModelTiersConfig } from '../types/generated';
import { compareTargetKey, type CompareTarget } from '../lib/compareTargets';
import './AgentCompareSelector.css';

export interface AgentCompareSelectorProps {
  /** The current selection, already resolved (no "everyone" placeholder). */
  targets: CompareTarget[];
  choices: AgentSwitchTarget[];
  unusable: UnusableExternalAgentTarget[];
  installedAgentTypes: AgentType[];
  defaultTier: ModelTier;
  /** Agent of a row added with "+ Ajouter". */
  addAgent: AgentType;
  modelTiers?: ModelTiersConfig | null;
  label?: ReactNode;
  testIdPrefix?: string;
  /** `null` selects every available choice again. */
  onChange: (targets: CompareTarget[] | null) => void;
}

/**
 * The multi-agent selector of the comparative Quick Prompt: one chip per
 * target with the shared agent+tier picker, unusable connections shown with
 * their reason, "+ Ajouter" and "Tout · Aucun".
 */
export function AgentCompareSelector({
  targets,
  choices,
  unusable,
  installedAgentTypes,
  defaultTier,
  addAgent,
  modelTiers,
  label,
  testIdPrefix = 'qp-compare',
  onChange,
}: AgentCompareSelectorProps) {
  const { t } = useT();
  const represented = new Set(targets.map(compareTargetKey));
  const missingChoices = choices.filter(choice =>
    !represented.has(compareTargetKey({
      agent: choice.agent,
      connection_id: choice.connectionId ?? undefined,
    }))
  );
  const hasOneOfEveryAgent = targets.length === choices.length && missingChoices.length === 0;

  return (
    <div className="qp-compare-selector">
      <span className="qp-compare-selector-label">
        {label ?? <>🤝 {t('qp.compareAgents.selectorLabel')}</>}
      </span>
      {targets.map((target, targetIndex) => {
        const selectedChoice = choices.find(choice =>
          choice.agent === target.agent
          && (choice.connectionId ?? undefined) === target.connection_id
        );
        const targetLabel = selectedChoice?.label ?? AGENT_LABELS[target.agent] ?? target.agent;
        const duplicateIndex = targets
          .slice(0, targetIndex)
          .filter(row => compareTargetKey(row) === compareTargetKey(target)).length;
        return (
          <span
            key={`${targetIndex}-${compareTargetKey(target)}-${target.tier}`}
            className="qp-compare-target qp-compare-target--active"
            style={{ borderColor: agentColor(target.agent) }}
          >
            <button
              type="button"
              data-testid={`${testIdPrefix}-chip-${target.connection_id ?? target.agent}${duplicateIndex > 0 ? `-${duplicateIndex + 1}` : ''}`}
              className="qp-compare-target-toggle"
              onClick={() => onChange(targets.filter((_, index) => index !== targetIndex))}
              aria-pressed="true"
              aria-label={`${t('qp.compareAgents.removeTarget')} ${targetLabel}`}
            >
              ✓
            </button>
            <AgentSwitchPicker
              currentAgent={target.agent}
              availableAgents={installedAgentTypes}
              currentConnectionId={target.connection_id}
              currentTargetLabel={targetLabel}
              availableTargets={choices}
              currentTier={target.tier}
              onTargetSelectionChange={async (choice, tier) => {
                onChange(targets.map((row, index) => index === targetIndex
                  ? {
                      agent: choice.agent,
                      tier,
                      ...(choice.connectionId ? { connection_id: choice.connectionId } : {}),
                    }
                  : row));
              }}
              tierLabels={{
                economy: t('disc.tier.economy'),
                default: t('disc.tier.default'),
                reasoning: t('disc.tier.reasoning'),
              }}
              modelTiers={modelTiers}
              defaultModelLabel={t('config.defaultModel')}
              title={t('disc.switchAgentAndTier')}
              ariaLabel={t('qp.compareAgents.targetLabel', targetLabel)}
              compact
            />
          </span>
        );
      })}
      {unusable.map(choice => (
        <span
          key={`unusable-${choice.connectionId}`}
          className="qp-compare-target qp-compare-target--unusable"
          data-testid={`${testIdPrefix}-chip-unusable-${choice.connectionId}`}
          title={t(`qp.compareAgents.unusable.${choice.reason}`)}
        >
          <span className="qp-compare-target-empty">
            {choice.label}
            <span className="qp-compare-target-reason">
              {t(`qp.compareAgents.unusable.${choice.reason}`)}
            </span>
          </span>
        </span>
      ))}
      {missingChoices.map(choice => {
        const choiceLabel = choice.label ?? AGENT_LABELS[choice.agent] ?? choice.agent;
        const key = choice.connectionId ?? choice.agent;
        return (
          <span key={`missing-${key}`} className="qp-compare-target qp-compare-target--inactive">
            <button
              type="button"
              data-testid={`${testIdPrefix}-chip-${key}`}
              className="qp-compare-target-toggle"
              onClick={() => onChange([...targets, {
                agent: choice.agent,
                tier: defaultTier,
                ...(choice.connectionId ? { connection_id: choice.connectionId } : {}),
              }])}
              aria-pressed="false"
              aria-label={`${t('qp.compareAgents.addTarget')} ${choiceLabel}`}
            >
              +
            </button>
            <span className="qp-compare-target-empty">
              {choiceLabel}
              <span aria-hidden="true">{MODEL_TIER_ICONS.default}</span>
            </span>
          </span>
        );
      })}
      <button
        type="button"
        className="qp-compare-add-model"
        onClick={() => onChange([...targets, { agent: addAgent, tier: defaultTier }])}
      >
        <Plus size={11} /> {t('qp.compareAgents.addModel')}
      </button>
      <button
        type="button"
        className="qp-compare-toggle-all"
        onClick={() => onChange(hasOneOfEveryAgent ? [] : null)}
      >
        {hasOneOfEveryAgent
          ? t('qp.compareAgents.selectNone')
          : t('qp.compareAgents.selectAll')}
      </button>
    </div>
  );
}

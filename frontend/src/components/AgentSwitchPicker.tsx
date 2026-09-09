import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Loader2, RefreshCw } from 'lucide-react';
import { modelCatalogApi } from '../lib/api';
import { catalogModelProvenance, catalogTierEntry, modelRuntimeTargetId } from '../lib/modelCatalogSelection';
import { useT } from '../lib/I18nContext';
import {
  AGENT_COLORS,
  AGENT_LABELS,
  MODEL_TIER_ICONS,
  agentTextColor,
  modelForAgentTier,
} from '../lib/constants';
import type {
  AgentType,
  ModelCatalogSnapshot,
  ModelTier,
  ModelTierConfig,
  ModelTiersConfig,
} from '../types/generated';
import './AgentSwitchPicker.css';

export interface AgentSwitchTarget {
  agent: AgentType;
  connectionId?: string | null;
  label?: string;
  modelTiers?: ModelTierConfig;
}

interface AgentSwitchPickerProps {
  currentAgent: AgentType;
  availableAgents: AgentType[];
  currentConnectionId?: string | null;
  currentTargetLabel?: string;
  availableTargets?: AgentSwitchTarget[];
  onChange?: (agent: AgentType) => Promise<void>;
  currentTier?: ModelTier;
  /** Saved per-discussion/QP override; alternative selections resolve their own tiers. */
  currentModel?: string | null;
  onSelectionChange?: (agent: AgentType, tier: ModelTier) => Promise<void>;
  onTargetSelectionChange?: (target: AgentSwitchTarget, tier: ModelTier) => Promise<void>;
  tierLabels?: Record<ModelTier, string>;
  modelTiers?: ModelTiersConfig | null;
  defaultModelLabel?: string;
  disabled?: boolean;
  compact?: boolean;
  title: string;
  ariaLabel: string;
  staticClassName?: string;
  suffix?: string;
  displayName?: string;
}

/**
 * Shared inline agent picker used by discussion headers and workflow steps.
 * It owns only the popover interaction; callers remain responsible for
 * persisting the selected agent and surfacing any API error.
 */
export function AgentSwitchPicker({
  currentAgent,
  availableAgents,
  currentConnectionId,
  currentTargetLabel,
  availableTargets,
  onChange,
  currentTier,
  currentModel,
  onSelectionChange,
  onTargetSelectionChange,
  tierLabels,
  modelTiers,
  defaultModelLabel = 'Default agent model',
  disabled = false,
  compact = false,
  title,
  ariaLabel,
  staticClassName,
  suffix,
  displayName,
}: AgentSwitchPickerProps) {
  const { t } = useT();
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [catalog, setCatalog] = useState<ModelCatalogSnapshot | null>(null);
  const [catalogError, setCatalogError] = useState(false);
  const [popoverPosition, setPopoverPosition] = useState<{ top: number; left: number } | null>(null);
  const savingRef = useRef(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const popoverRef = useRef<HTMLSpanElement>(null);
  const targetKey = (target: AgentSwitchTarget) => `${target.agent}:${target.connectionId ?? ''}`;
  const suppliedTargets: AgentSwitchTarget[] = availableTargets
    ?? availableAgents.map(agent => ({ agent }));
  const suppliedCurrent = suppliedTargets.find(target =>
    target.agent === currentAgent
    && (target.connectionId ?? null) === (currentConnectionId ?? null)
  );
  const currentTarget: AgentSwitchTarget = suppliedCurrent ?? {
    agent: currentAgent,
    connectionId: currentConnectionId,
    label: currentTargetLabel,
  };
  const choices = [currentTarget, ...suppliedTargets].filter((target, index, targets) =>
    targets.findIndex(candidate => targetKey(candidate) === targetKey(target)) === index
  );
  const choiceCount = choices.length;
  const tierChoices: ModelTier[] = ['economy', 'default', 'reasoning'];
  const tierPicker = currentTier !== undefined
    && (onSelectionChange !== undefined || onTargetSelectionChange !== undefined);
  const canChange = tierPicker
    ? choices.length > 1 || tierChoices.length > 1
    : Boolean(onChange) && choices.length > 1;

  const targetLabel = (target: AgentSwitchTarget) =>
    target.label ?? AGENT_LABELS[target.agent] ?? target.agent;
  const runtimeTargetId = (target: AgentSwitchTarget) => modelRuntimeTargetId(target.agent, target.connectionId);
  const catalogView = (target: AgentSwitchTarget) =>
    catalog?.targets.find(view => view.runtime_target_id === runtimeTargetId(target));
  const isHttpTarget = (target: AgentSwitchTarget) => Boolean(target.connectionId)
    || ['Ollama', 'LiteLlm', 'Nvidia'].includes(target.agent);
  const configuredModel = (target: AgentSwitchTarget, tier: ModelTier) => {
    if (currentModel?.trim() && targetKey(target) === targetKey(currentTarget) && tier === currentTier) {
      return currentModel;
    }
    const configured = target.modelTiers?.[tier]
      || (isHttpTarget(target) ? target.modelTiers?.default : null);
    // A named connection owns its configuration; family defaults belong to a different target.
    return configured || (target.connectionId ? '' : modelForAgentTier(target.agent, tier, modelTiers, ''));
  };
  const catalogEntry = (target: AgentSwitchTarget, tier: ModelTier) =>
    catalogTierEntry(catalogView(target), tier, configuredModel(target, tier), isHttpTarget(target));
  const targetModel = (target: AgentSwitchTarget, tier: ModelTier) => {
    const entry = catalogEntry(target, tier);
    return entry?.display_alias ?? entry?.display_name ?? entry?.model_id
      ?? (configuredModel(target, tier) || defaultModelLabel);
  };
  const tierTitle = (target: AgentSwitchTarget, tier: ModelTier) =>
    `${tierLabels?.[tier] ?? tier} · ${targetModel(target, tier)}`;
  const effectiveSuffix = suffix
    ?? (tierPicker && currentTier ? targetModel(currentTarget, currentTier) : undefined);

  const updatePopoverPosition = useCallback(() => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;

    const viewportPadding = 8;
    const compactTierLayout = tierPicker && window.innerWidth < 420;
    const popoverWidth = tierPicker ? 342 : 170;
    const estimatedHeight = choiceCount * (compactTierLayout ? 75 : tierPicker ? 45 : 31) + 8;
    const hasRoomBelow = rect.bottom + 5 + estimatedHeight <= window.innerHeight - viewportPadding;
    const top = hasRoomBelow
      ? rect.bottom + 5
      : Math.max(viewportPadding, rect.top - estimatedHeight - 5);
    const left = Math.max(
      viewportPadding,
      Math.min(rect.left, window.innerWidth - popoverWidth - viewportPadding),
    );
    setPopoverPosition({ top, left });
  }, [choiceCount, tierPicker]);

  const select = async (target: AgentSwitchTarget, tier?: ModelTier) => {
    if (savingRef.current) return;
    if (tierPicker) {
      if (!tier || (
        targetKey(target) === targetKey(currentTarget)
        && tier === currentTier
      )) return;
    } else if (!onChange || target.agent === currentAgent) {
      return;
    }
    savingRef.current = true;
    setSaving(true);
    try {
      if (tierPicker && tier && onTargetSelectionChange) {
        await onTargetSelectionChange(target, tier);
      } else if (tierPicker && tier && onSelectionChange) {
        await onSelectionChange(target.agent, tier);
      } else if (onChange) {
        await onChange(target.agent);
      }
      setOpen(false);
    } catch {
      // The caller owns the user-facing error. Keep the picker open so
      // another available combination can be selected.
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  useEffect(() => {
    if (!open) return;
    let active = true;
    void modelCatalogApi.list()
      .then(snapshot => {
        if (active) {
          setCatalog(snapshot);
          setCatalogError(false);
        }
      })
      .catch(() => {
        if (!active) return;
        setCatalogError(true);
        // Keep known identities, but do not present the previous snapshot as fresh.
        setCatalog(previous => previous ? {
          ...previous,
          targets: previous.targets.map(target => ({ ...target, stale: true, live_refresh_ok: false })),
        } : null);
      });
    updatePopoverPosition();
    const closeOutside = (event: MouseEvent) => {
      const target = event.target as Node;
      if (
        !rootRef.current?.contains(target)
        && !popoverRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', closeOutside);
    document.addEventListener('keydown', closeOnEscape);
    window.addEventListener('resize', updatePopoverPosition);
    window.addEventListener('scroll', updatePopoverPosition, true);
    return () => {
      active = false;
      document.removeEventListener('mousedown', closeOutside);
      document.removeEventListener('keydown', closeOnEscape);
      window.removeEventListener('resize', updatePopoverPosition);
      window.removeEventListener('scroll', updatePopoverPosition, true);
    };
  }, [open, updatePopoverPosition]);

  if (!canChange) {
    return (
      <span
        className={staticClassName ?? 'kr-agent-switch-static'}
        style={{ color: agentTextColor(currentTarget.agent) }}
      >
        {displayName ?? targetLabel(currentTarget)}
        {effectiveSuffix && <span className="kr-agent-switch-suffix"> · {effectiveSuffix}</span>}
      </span>
    );
  }

  return (
    <span
      ref={rootRef}
      className="kr-agent-switch"
      data-compact={compact}
      data-open={open}
      onClick={event => event.stopPropagation()}
    >
      <button
        type="button"
        className="kr-agent-switch-btn"
        style={{ color: agentTextColor(currentTarget.agent) }}
        title={tierPicker && currentTier
          ? `${title} · ${tierTitle(currentTarget, currentTier)}`
          : title}
        aria-label={ariaLabel}
        aria-expanded={open}
        disabled={disabled || saving}
        onClick={() => {
          if (!open) updatePopoverPosition();
          setOpen(value => !value);
        }}
      >
        {saving ? <Loader2 size={9} className="spin" /> : <RefreshCw size={9} />}
        <span>{displayName ?? targetLabel(currentTarget)}</span>
        {tierPicker && currentTier && (
          <span
            className="kr-agent-switch-current-tier"
            data-tier={currentTier}
            title={tierTitle(currentTarget, currentTier)}
          >
            {MODEL_TIER_ICONS[currentTier]}
          </span>
        )}
        {effectiveSuffix && <span className="kr-agent-switch-suffix"> · {effectiveSuffix}</span>}
        <ChevronDown size={9} />
      </button>
      {open && popoverPosition && createPortal(
        <span
          ref={popoverRef}
          className="kr-agent-switch-popover"
          role="menu"
          style={popoverPosition}
        >
          {catalogError && <span role="status" className="kr-agent-switch-catalog-meta">{t('modelCatalog.loadError')}</span>}
          {choices.map(target => tierPicker ? (
            <span
              key={targetKey(target)}
              className="kr-agent-switch-tier-row"
              role="group"
              aria-label={targetLabel(target)}
            >
              <span className="kr-agent-switch-tier-agent">
                <span
                  className="kr-agent-switch-option-dot"
                  style={{ background: AGENT_COLORS[target.agent] ?? 'var(--kr-text-faint)' }}
                />
                {targetLabel(target)}
              </span>
              <span className="kr-agent-switch-tier-choices">
                {tierChoices.map(tier => {
                  const selected = targetKey(target) === targetKey(currentTarget)
                    && tier === currentTier;
                  const icon = MODEL_TIER_ICONS[tier];
                  const label = tierLabels?.[tier] ?? tier;
                  const entry = catalogEntry(target, tier);
                  const unavailable = entry?.availability === 'unavailable';
                  const provenance = entry
                    ? t(`modelCatalog.provenance.${catalogModelProvenance(entry, catalogView(target))}`)
                    : null;
                  return (
                    <button
                      key={tier}
                      type="button"
                      role="menuitem"
                      className="kr-agent-switch-tier-option"
                      data-tier={tier}
                      data-current={selected}
                      aria-label={`${targetLabel(target)} · ${label}`}
                      title={tierTitle(target, tier)}
                      disabled={saving || selected || unavailable}
                      onClick={() => void select(target, tier)}
                    >
                      <span aria-hidden="true">{icon}</span>
                      <span>{label}</span>
                      {(entry || configuredModel(target, tier)) && (
                        <span className="kr-agent-switch-catalog-meta">
                          {unavailable
                            ? t('modelCatalog.unavailable')
                            : provenance ?? t('modelCatalog.notInCatalog')}
                        </span>
                      )}
                      {selected && <Check size={8} aria-hidden="true" />}
                    </button>
                  );
                })}
              </span>
            </span>
          ) : (
            <button
              key={targetKey(target)}
              type="button"
              role="menuitem"
              className="kr-agent-switch-option"
              data-current={targetKey(target) === targetKey(currentTarget)}
              disabled={saving || targetKey(target) === targetKey(currentTarget)}
              onClick={() => void select(target)}
            >
              <span
                className="kr-agent-switch-option-dot"
                style={{ background: AGENT_COLORS[target.agent] ?? 'var(--kr-text-faint)' }}
              />
              {targetLabel(target)}
              {targetKey(target) === targetKey(currentTarget) && <Check size={10} />}
            </button>
          ))}
        </span>,
        document.body,
      )}
    </span>
  );
}

import { useCallback, useEffect, useId, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Loader2, RefreshCw } from 'lucide-react';
import { resolveCatalogTier, modelRuntimeTargetId } from '../lib/modelCatalogSelection';
import { useModelCatalogSnapshot } from '../hooks/useModelCatalogSnapshot';
import { useT } from '../lib/I18nContext';
import {
  AGENT_COLORS,
  AGENT_LABELS,
  MODEL_TIER_ICONS,
  agentTextColor,
} from '../lib/constants';
import type {
  AgentType,
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
  const pickerId = useId();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [saving, setSaving] = useState(false);
  const { catalog, catalogError } = useModelCatalogSnapshot(open);
  const [popoverPosition, setPopoverPosition] = useState<{ top: number; left: number; maxHeight: number } | null>(null);
  const savingRef = useRef(false);
  const restoreFocusRef = useRef(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const popoverRef = useRef<HTMLSpanElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
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
  const resolvedTier = (target: AgentSwitchTarget, tier: ModelTier) => resolveCatalogTier(
    catalog, target, tier, modelTiers,
    targetKey(target) === targetKey(currentTarget) && tier === currentTier ? currentModel : null,
  );
  const configuredModel = (target: AgentSwitchTarget, tier: ModelTier) => resolvedTier(target, tier).configured;
  const catalogEntry = (target: AgentSwitchTarget, tier: ModelTier) =>
    resolvedTier(target, tier).entry;
  const targetModel = (target: AgentSwitchTarget, tier: ModelTier) =>
    resolvedTier(target, tier).model || defaultModelLabel;
  const tierTitle = (target: AgentSwitchTarget, tier: ModelTier) =>
    `${tierLabels?.[tier] ?? tier} · ${targetModel(target, tier)}`;
  const effectiveSuffix = suffix
    ?? (tierPicker && currentTier ? targetModel(currentTarget, currentTier) : undefined);
  const searchText = (value: string) => value.normalize('NFKD').replace(/\p{Diacritic}/gu, '').toLowerCase();
  const normalizedQuery = searchText(query.trim());
  const visibleChoices = choices.filter(target => !normalizedQuery || searchText([
    targetLabel(target), target.agent, target.connectionId, runtimeTargetId(target),
    ...tierChoices.flatMap(tier => {
      const entry = catalogEntry(target, tier);
      return [tier, tierLabels?.[tier], configuredModel(target, tier),
        entry?.model_id, entry?.display_name, entry?.display_alias];
    }),
  ].filter(Boolean).join(' ')).includes(normalizedQuery));

  const navigateChoices = (event: ReactKeyboardEvent<HTMLSpanElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      restoreFocusRef.current = true;
      setOpen(false);
      return;
    }
    const options = Array.from(popoverRef.current?.querySelectorAll<HTMLButtonElement>(
      '[role="menuitem"]:not(:disabled)',
    ) ?? []);
    if (!['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
    const inSearch = event.target === searchRef.current;
    // Home/End keep their normal caret behavior in the search field.
    if (inSearch && (event.key === 'Home' || event.key === 'End')) return;
    event.preventDefault();
    event.stopPropagation();
    if (!options.length) return;
    const index = options.indexOf(event.target as HTMLButtonElement);
    if (event.key === 'Home') options[0].focus();
    else if (event.key === 'End') options[options.length - 1].focus();
    else if (event.key === 'ArrowUp' && index === 0) searchRef.current?.focus();
    else if (event.key === 'ArrowUp') options[index < 0 ? options.length - 1 : index - 1].focus();
    else options[(index + 1) % options.length].focus();
  };

  const updatePopoverPosition = useCallback(() => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;

    const viewportPadding = 8;
    const compactTierLayout = tierPicker && window.innerWidth < 420;
    const popoverWidth = tierPicker ? 342 : 170;
    const estimatedHeight = choiceCount * (compactTierLayout ? 75 : tierPicker ? 45 : 31) + 46;
    const hasRoomBelow = rect.bottom + 5 + estimatedHeight <= window.innerHeight - viewportPadding;
    const top = hasRoomBelow
      ? rect.bottom + 5
      : Math.max(viewportPadding, rect.top - estimatedHeight - 5);
    const left = Math.max(
      viewportPadding,
      Math.min(rect.left, window.innerWidth - popoverWidth - viewportPadding),
    );
    setPopoverPosition({ top, left, maxHeight: Math.max(0, window.innerHeight - top - viewportPadding) });
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
      restoreFocusRef.current = true;
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
    if (!open && !saving && restoreFocusRef.current) {
      restoreFocusRef.current = false;
      rootRef.current?.querySelector('button')?.focus();
    }
  }, [open, saving]);

  useEffect(() => {
    if (!open) return;
    searchRef.current?.focus();
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
      if (event.key === 'Escape' && !event.isComposing) {
        restoreFocusRef.current = true;
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', closeOutside);
    document.addEventListener('keydown', closeOnEscape);
    window.addEventListener('resize', updatePopoverPosition);
    window.addEventListener('scroll', updatePopoverPosition, true);
    return () => {
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
        aria-haspopup="dialog"
        aria-expanded={open}
        disabled={disabled || saving}
        onClick={() => {
          if (!open) {
            setQuery('');
            updatePopoverPosition();
          }
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
          role="dialog"
          aria-label={ariaLabel}
          style={popoverPosition}
          onKeyDown={navigateChoices}
        >
          <input
            ref={searchRef}
            type="search"
            className="kr-agent-switch-search"
            aria-label={t('agentPicker.search')}
            placeholder={t('agentPicker.search')}
            value={query}
            onChange={event => setQuery(event.target.value)}
          />
          {catalogError && <span role="status" className="kr-agent-switch-status">{t('modelCatalog.loadError')}</span>}
          {!visibleChoices.length && <span role="status" className="kr-agent-switch-status">{t('agentPicker.noMatch')}</span>}
          <span role="menu" aria-label={ariaLabel} className="kr-agent-switch-results">
          {visibleChoices.map((target, targetIndex) => tierPicker ? (
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
                    ? t(`modelCatalog.provenance.${resolvedTier(target, tier).provenance}`)
                    : null;
                  const descriptionId = `${pickerId}-${targetIndex}-${tier}`;
                  return (
                    <button
                      key={tier}
                      type="button"
                      role="menuitem"
                      className="kr-agent-switch-tier-option"
                      data-tier={tier}
                      data-current={selected}
                      aria-label={`${targetLabel(target)} · ${label}`}
                      aria-describedby={descriptionId}
                      title={tierTitle(target, tier)}
                      disabled={saving || selected || unavailable}
                      onClick={() => void select(target, tier)}
                    >
                      <span aria-hidden="true">{icon}</span>
                      <span>{label}</span>
                      <span id={descriptionId} hidden>
                        {[tierTitle(target, tier), provenance ?? (configuredModel(target, tier) ? t('modelCatalog.notInCatalog') : ''),
                          unavailable ? t('modelCatalog.unavailable') : ''].filter(Boolean).join(' · ')}
                      </span>
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
          </span>
        </span>,
        document.body,
      )}
    </span>
  );
}

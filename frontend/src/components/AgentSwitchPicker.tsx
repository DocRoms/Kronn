import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Loader2, RefreshCw } from 'lucide-react';
import {
  AGENT_COLORS,
  AGENT_LABELS,
  MODEL_TIER_ICONS,
  modelForAgentTier,
} from '../lib/constants';
import type { AgentType, ModelTier, ModelTiersConfig } from '../types/generated';
import './AgentSwitchPicker.css';

interface AgentSwitchPickerProps {
  currentAgent: AgentType;
  availableAgents: AgentType[];
  onChange?: (agent: AgentType) => Promise<void>;
  currentTier?: ModelTier;
  onSelectionChange?: (agent: AgentType, tier: ModelTier) => Promise<void>;
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
  onChange,
  currentTier,
  onSelectionChange,
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
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [popoverPosition, setPopoverPosition] = useState<{ top: number; left: number } | null>(null);
  const savingRef = useRef(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const popoverRef = useRef<HTMLSpanElement>(null);
  const choices = Array.from(new Set<AgentType>([currentAgent, ...availableAgents]));
  const tierChoices: ModelTier[] = ['economy', 'default', 'reasoning'];
  const tierPicker = currentTier !== undefined && onSelectionChange !== undefined;
  const canChange = tierPicker
    ? choices.length > 1 || tierChoices.length > 1
    : Boolean(onChange) && choices.length > 1;

  const tierTitle = (agent: AgentType, tier: ModelTier) =>
    `${tierLabels?.[tier] ?? tier} · ${modelForAgentTier(agent, tier, modelTiers, defaultModelLabel)}`;

  const updatePopoverPosition = useCallback(() => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;

    const viewportPadding = 8;
    const compactTierLayout = tierPicker && window.innerWidth < 420;
    const popoverWidth = tierPicker ? 342 : 170;
    const estimatedHeight = choices.length * (compactTierLayout ? 75 : tierPicker ? 45 : 31) + 8;
    const hasRoomBelow = rect.bottom + 5 + estimatedHeight <= window.innerHeight - viewportPadding;
    const top = hasRoomBelow
      ? rect.bottom + 5
      : Math.max(viewportPadding, rect.top - estimatedHeight - 5);
    const left = Math.max(
      viewportPadding,
      Math.min(rect.left, window.innerWidth - popoverWidth - viewportPadding),
    );
    setPopoverPosition({ top, left });
  }, [choices.length, tierPicker]);

  const select = async (agent: AgentType, tier?: ModelTier) => {
    if (savingRef.current) return;
    if (tierPicker) {
      if (!tier || (agent === currentAgent && tier === currentTier)) return;
    } else if (!onChange || agent === currentAgent) {
      return;
    }
    savingRef.current = true;
    setSaving(true);
    try {
      if (tierPicker && tier) await onSelectionChange(agent, tier);
      else if (onChange) await onChange(agent);
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
        style={{ color: AGENT_COLORS[currentAgent] ?? 'var(--kr-text-faint)' }}
      >
        {displayName ?? AGENT_LABELS[currentAgent] ?? currentAgent}
        {suffix && <span className="kr-agent-switch-suffix"> · {suffix}</span>}
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
        style={{ color: AGENT_COLORS[currentAgent] ?? 'var(--kr-text-faint)' }}
        title={tierPicker && currentTier
          ? `${title} · ${tierTitle(currentAgent, currentTier)}`
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
        <span>{displayName ?? AGENT_LABELS[currentAgent] ?? currentAgent}</span>
        {tierPicker && currentTier && (
          <span
            className="kr-agent-switch-current-tier"
            data-tier={currentTier}
            title={tierTitle(currentAgent, currentTier)}
          >
            {MODEL_TIER_ICONS[currentTier]}
          </span>
        )}
        {suffix && <span className="kr-agent-switch-suffix"> · {suffix}</span>}
        <ChevronDown size={9} />
      </button>
      {open && popoverPosition && createPortal(
        <span
          ref={popoverRef}
          className="kr-agent-switch-popover"
          role="menu"
          style={popoverPosition}
        >
          {choices.map(agent => tierPicker ? (
            <span
              key={agent}
              className="kr-agent-switch-tier-row"
              role="group"
              aria-label={AGENT_LABELS[agent] ?? agent}
            >
              <span className="kr-agent-switch-tier-agent">
                <span
                  className="kr-agent-switch-option-dot"
                  style={{ background: AGENT_COLORS[agent] ?? 'var(--kr-text-faint)' }}
                />
                {AGENT_LABELS[agent] ?? agent}
              </span>
              <span className="kr-agent-switch-tier-choices">
                {tierChoices.map(tier => {
                  const selected = agent === currentAgent && tier === currentTier;
                  const icon = MODEL_TIER_ICONS[tier];
                  const label = tierLabels?.[tier] ?? tier;
                  return (
                    <button
                      key={tier}
                      type="button"
                      role="menuitem"
                      className="kr-agent-switch-tier-option"
                      data-tier={tier}
                      data-current={selected}
                      aria-label={`${AGENT_LABELS[agent] ?? agent} · ${label}`}
                      title={tierTitle(agent, tier)}
                      disabled={saving || selected}
                      onClick={() => void select(agent, tier)}
                    >
                      <span aria-hidden="true">{icon}</span>
                      <span>{label}</span>
                      {selected && <Check size={8} aria-hidden="true" />}
                    </button>
                  );
                })}
              </span>
            </span>
          ) : (
            <button
              key={agent}
              type="button"
              role="menuitem"
              className="kr-agent-switch-option"
              data-current={agent === currentAgent}
              disabled={saving || agent === currentAgent}
              onClick={() => void select(agent)}
            >
              <span
                className="kr-agent-switch-option-dot"
                style={{ background: AGENT_COLORS[agent] ?? 'var(--kr-text-faint)' }}
              />
              {AGENT_LABELS[agent] ?? agent}
              {agent === currentAgent && <Check size={10} />}
            </button>
          ))}
        </span>,
        document.body,
      )}
    </span>
  );
}

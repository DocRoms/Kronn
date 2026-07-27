import { useCallback, useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, Loader2, RefreshCw } from 'lucide-react';
import { AGENT_COLORS, AGENT_LABELS } from '../lib/constants';
import type { AgentType } from '../types/generated';
import './AgentSwitchPicker.css';

interface AgentSwitchPickerProps {
  currentAgent: AgentType;
  availableAgents: AgentType[];
  onChange?: (agent: AgentType) => Promise<void>;
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
  const canChange = Boolean(onChange) && choices.length > 1;

  const updatePopoverPosition = useCallback(() => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;

    const viewportPadding = 8;
    const popoverWidth = 170;
    const estimatedHeight = choices.length * 31 + 8;
    const hasRoomBelow = rect.bottom + 5 + estimatedHeight <= window.innerHeight - viewportPadding;
    const top = hasRoomBelow
      ? rect.bottom + 5
      : Math.max(viewportPadding, rect.top - estimatedHeight - 5);
    const left = Math.max(
      viewportPadding,
      Math.min(rect.left, window.innerWidth - popoverWidth - viewportPadding),
    );
    setPopoverPosition({ top, left });
  }, [choices.length]);

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
        title={title}
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
          {choices.map(agent => (
            <button
              key={agent}
              type="button"
              role="menuitem"
              className="kr-agent-switch-option"
              data-current={agent === currentAgent}
              disabled={saving || agent === currentAgent}
              onClick={async () => {
                if (!onChange || savingRef.current || agent === currentAgent) return;
                savingRef.current = true;
                setSaving(true);
                try {
                  await onChange(agent);
                  setOpen(false);
                } catch {
                  // The caller owns the user-facing error. Keep the picker
                  // open so another available agent can be selected.
                } finally {
                  savingRef.current = false;
                  setSaving(false);
                }
              }}
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

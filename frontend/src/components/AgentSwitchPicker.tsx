import { useEffect, useRef, useState } from 'react';
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
}: AgentSwitchPickerProps) {
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const savingRef = useRef(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const choices = Array.from(new Set<AgentType>([currentAgent, ...availableAgents]));
  const canChange = Boolean(onChange) && choices.length > 1;

  useEffect(() => {
    if (!open) return;
    const closeOutside = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', closeOutside);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('mousedown', closeOutside);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [open]);

  if (!canChange) {
    return (
      <span
        className={staticClassName ?? 'kr-agent-switch-static'}
        style={{ color: AGENT_COLORS[currentAgent] ?? 'var(--kr-text-faint)' }}
      >
        {AGENT_LABELS[currentAgent] ?? currentAgent}
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
        onClick={() => setOpen(value => !value)}
      >
        {saving ? <Loader2 size={9} className="spin" /> : <RefreshCw size={9} />}
        <span>{AGENT_LABELS[currentAgent] ?? currentAgent}</span>
        <ChevronDown size={9} />
      </button>
      {open && (
        <span className="kr-agent-switch-popover" role="menu">
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
        </span>
      )}
    </span>
  );
}

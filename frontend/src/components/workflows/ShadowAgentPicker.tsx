/**
 * KT-1696 — the agent/model menu of a shadow (and of the merge's condense
 * agent): one click opens the agents grouped with their transport, each with the
 * models it offers. « = principal » picks the step's own agent, which makes an
 * identical shadow. The step's own model is greyed out: that is what « = principal »
 * already says.
 */
import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { Check, ChevronDown } from 'lucide-react';
import { useModelCatalogSnapshot } from '../../hooks/useModelCatalogSnapshot';
import { AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES } from '../../lib/constants';
import { modelRuntimeTargetId } from '../../lib/modelCatalogSelection';
import { agentTransport, type AgentTransport } from '../../lib/shadowMerge';
import type { AgentSwitchTarget } from '../AgentSwitchPicker';
import { agentModelLabel } from './shadowRowModel';
import type { AgentType, ShadowAgent } from '../../types/generated';

type Translate = (key: string, ...args: (string | number)[]) => string;

/** CLI / HTTP / Local, the badge the agent lists carry. */
export function TransportBadge({ agent, connectionId, t }: { agent: string; connectionId?: string | null; t: Translate }) {
  const transport: AgentTransport = agentTransport(agent, connectionId);
  return <span className="wf-shadow-transport" data-transport={transport}>{t(`wf.shadow.rows.transport.${transport}`)}</span>;
}

/** The agent a step runs on, for what the menu greys out. */
export interface MainAgent {
  agent: AgentType;
  connectionId?: string | null;
  model?: string | null;
}

interface Props {
  /** `undefined` is « = step agent ». */
  value: ShadowAgent | undefined;
  onChange: (next: ShadowAgent | undefined) => void;
  choices?: readonly AgentSwitchTarget[];
  main: MainAgent;
  ariaLabel: string;
  testId: string;
  t: Translate;
  disabled?: boolean;
  /** The trigger's content; the current agent and model by default. */
  children?: ReactNode;
  className?: string;
}

/** The agents offered when the page supplies none: every agent Kronn knows. */
const FALLBACK_TARGETS: readonly AgentSwitchTarget[] = [...ALL_AGENT_TYPES, 'Custom' as AgentType].map(agent => ({ agent }));

const targetLabel = (target: AgentSwitchTarget) => target.label ?? AGENT_LABELS[target.agent] ?? target.agent;

export function ShadowAgentPicker({ value, onChange, choices, main, ariaLabel, testId, t, disabled = false, children, className }: Props) {
  const [open, setOpen] = useState(false);
  const [position, setPosition] = useState<{ top: number; left: number; maxHeight: number } | null>(null);
  const rootRef = useRef<HTMLSpanElement>(null);
  const popoverRef = useRef<HTMLDivElement>(null);
  const { catalog } = useModelCatalogSnapshot(open);
  const targets = choices?.length ? choices : FALLBACK_TARGETS;

  const place = useCallback(() => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (!rect) return;
    const pad = 8;
    const width = 300;
    const top = rect.bottom + 4;
    setPosition({
      top,
      left: Math.max(pad, Math.min(rect.left, window.innerWidth - width - pad)),
      maxHeight: Math.max(160, window.innerHeight - top - pad),
    });
  }, []);

  useEffect(() => {
    if (!open) return;
    place();
    const closeOutside = (event: MouseEvent) => {
      const node = event.target as Node;
      if (!rootRef.current?.contains(node) && !popoverRef.current?.contains(node)) setOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === 'Escape') setOpen(false); };
    document.addEventListener('mousedown', closeOutside);
    document.addEventListener('keydown', closeOnEscape);
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    return () => {
      document.removeEventListener('mousedown', closeOutside);
      document.removeEventListener('keydown', closeOnEscape);
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
    };
  }, [open, place]);

  const pick = (next: ShadowAgent | undefined) => {
    setOpen(false);
    onChange(next);
  };
  const isMain = (agent: AgentType, connectionId: string | null, model: string | null) =>
    agent === main.agent && connectionId === (main.connectionId ?? null) && (model ?? '') === (main.model ?? '');
  const isCurrent = (agent: AgentType, connectionId: string | null, model: string | null) =>
    !!value && value.agent === agent && (value.connection_id ?? null) === connectionId && (value.model ?? '') === (model ?? '');

  return (
    <span ref={rootRef} className={`wf-shadow-picker${className ? ` ${className}` : ''}`} onClick={event => event.stopPropagation()}>
      <button
        type="button"
        className="wf-shadow-picker-btn"
        data-testid={testId}
        aria-label={ariaLabel}
        aria-haspopup="menu"
        aria-expanded={open}
        disabled={disabled}
        onClick={() => setOpen(current => !current)}
      >
        {children ?? <span>{agentModelLabel(value, main, targets, t)}</span>}
        {!children && !value && <em className="wf-shadow-picker-hint">{t('wf.shadow.rows.sameAgent')}</em>}
        <ChevronDown size={9} aria-hidden />
      </button>
      {open && position && createPortal(
        <div ref={popoverRef} className="wf-shadow-menu" role="menu" aria-label={ariaLabel} style={position} data-testid={`${testId}-menu`}>
          <button
            type="button"
            role="menuitem"
            className="wf-shadow-menu-item"
            data-current={!value}
            data-testid={`${testId}-same`}
            onClick={() => pick(undefined)}
          >
            <span>{t('wf.shadow.rows.sameAgent')}</span>
            <span className="wf-shadow-menu-hint">{t('wf.shadow.rows.sameAgentHint')}</span>
            {!value && <Check size={10} aria-hidden />}
          </button>
          {targets.map(target => {
            const connectionId = target.connectionId ?? null;
            const models = catalog?.targets.find(view => view.runtime_target_id === modelRuntimeTargetId(target.agent, connectionId))?.models ?? [];
            const ids = models.map(model => model.model_id);
            const extra = value && value.agent === target.agent && (value.connection_id ?? null) === connectionId && value.model && !ids.includes(value.model)
              ? [value.model] : [];
            const options: { model: string | null; label: string }[] = [
              { model: null, label: t('wf.shadow.rows.defaultModel') },
              ...extra.map(model => ({ model, label: model })),
              ...models.map(model => ({ model: model.model_id, label: model.display_alias ?? model.display_name ?? model.model_id })),
            ];
            return (
              <div key={`${target.agent}:${connectionId ?? ''}`} className="wf-shadow-menu-group" role="group" aria-label={targetLabel(target)}>
                <div className="wf-shadow-menu-head">
                  <span className="wf-shadow-menu-dot" style={{ background: AGENT_COLORS[target.agent] ?? 'var(--kr-text-faint)' }} />
                  <span>{targetLabel(target)}</span>
                  <TransportBadge agent={target.agent} connectionId={connectionId} t={t} />
                </div>
                {options.map(option => {
                  const onMain = isMain(target.agent, connectionId, option.model);
                  const current = isCurrent(target.agent, connectionId, option.model);
                  return (
                    <button
                      key={option.model ?? ''}
                      type="button"
                      role="menuitem"
                      className="wf-shadow-menu-item"
                      data-current={current}
                      data-main={onMain}
                      disabled={onMain}
                      title={onMain ? t('wf.shadow.rows.mainModel') : undefined}
                      aria-label={`${targetLabel(target)} · ${option.label}`}
                      onClick={() => pick({
                        agent: target.agent,
                        ...(option.model ? { model: option.model } : {}),
                        ...(connectionId ? { connection_id: connectionId } : {}),
                      })}
                    >
                      <span>{option.label}</span>
                      {current && <Check size={10} aria-hidden />}
                    </button>
                  );
                })}
              </div>
            );
          })}
        </div>,
        document.body,
      )}
    </span>
  );
}

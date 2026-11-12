import { useEffect, useRef, useState, type ReactNode } from 'react';
import { AlertTriangle, ArrowRight, Plus, X } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { AGENT_LABELS } from '../../lib/constants';
import type { AgentFallback, AgentType, ChainView, FallbackMode, FallbackTransport } from '../../types/generated';
import {
  TRANSPORT_LABEL,
  chainProblem,
  linkMode,
  modeKind,
  optionsFor,
  toLink,
  transportOf,
  type FallbackOption,
  type InheritedLink,
} from '../../lib/fallbackChain';
import './FallbackRow.css';

const label = (agent: AgentType) => AGENT_LABELS[agent] ?? agent;

/** The badge every pill carries: how the model is reached. There is no « paid » badge. */
export function TransportBadge({ transport }: { transport: FallbackTransport }) {
  return (
    <span className="fbp-badge" data-transport={transport} data-testid={`fallback-transport-${transport}`}>
      {TRANSPORT_LABEL[transport]}
    </span>
  );
}

interface PillProps {
  testId: string;
  agent: AgentType;
  model: string | null | undefined;
  transport: FallbackTransport;
  /** « eco », « advanced »…: shown when the model serves another mode than its row. */
  mode?: FallbackMode;
  gaps?: string[];
  /** `current`: the model the mode runs on; `own`: a fallback set here; `inherited`: set on another card. */
  tone: 'current' | 'own' | 'inherited';
  title?: string;
  disabled?: boolean;
  onClick?: () => void;
  onRemove?: () => void;
  removeLabel?: string;
}

/** `Agent · model` and its transport badge. A pill that can be clicked changes
 *  it (or, for an inherited one, opens the card that holds it); ✕ removes it. */
export function FallbackPill({
  testId, agent, model, transport, mode, gaps = [], tone, title, disabled, onClick, onRemove, removeLabel,
}: PillProps) {
  const { t } = useT();
  const body = (
    <>
      <span className="fbp-agent">{label(agent)}</span>
      <span className="fbp-model">{model ?? t('fallback.modelDefault')}</span>
      {mode && <span className="fbp-mode">{t(`fallback.mode.${mode}`)}</span>}
      <TransportBadge transport={transport} />
      {gaps.length > 0 && (
        <span className="fbp-gap" title={t('fallback.gaps', label(agent), gaps.join(', '))}>
          <AlertTriangle size={10} aria-hidden="true" />
        </span>
      )}
    </>
  );
  return (
    <span className="fbp" data-tone={tone} data-testid={testId}>
      {onClick ? (
        <button type="button" className="fbp-main" title={title} disabled={disabled} onClick={onClick}>{body}</button>
      ) : (
        <span className="fbp-main" title={title}>{body}</span>
      )}
      {onRemove && (
        <button
          type="button"
          className="fbp-x"
          disabled={disabled}
          title={removeLabel}
          aria-label={removeLabel}
          data-testid={`${testId}-remove`}
          onClick={onRemove}
        >
          <X size={10} aria-hidden="true" />
        </button>
      )}
    </span>
  );
}

interface MenuProps {
  testId: string;
  options: FallbackOption[];
  /** Agents the walk already goes through: never offered again (a chain never has an agent twice). */
  taken: AgentType[];
  /** The target being changed, if any: marked, not disabled. */
  selected?: AgentFallback;
  replaced: FallbackMode;
  onPick: (option: FallbackOption) => void;
  onClose: () => void;
}

/** The options of a row grouped by agent, each with its model, its mode and its transport. */
function FallbackMenu({ testId, options, taken, selected, replaced, onPick, onClose }: MenuProps) {
  const { t } = useT();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const away = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) onClose();
    };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', escape);
    };
  }, [onClose]);

  const groups: { agent: AgentType; items: FallbackOption[] }[] = [];
  for (const option of options) {
    const group = groups.find((entry) => entry.agent === option.agent);
    if (group) group.items.push(option);
    else groups.push({ agent: option.agent, items: [option] });
  }
  return (
    <div className="fbm" role="menu" aria-label={t('fallback.menu')} ref={ref} data-testid={testId}>
      {groups.length === 0 && <div className="fbm-empty">{t('fallback.menuEmpty')}</div>}
      {groups.map((group) => (
        <div key={group.agent} className="fbm-group" role="group" aria-label={label(group.agent)}>
          <div className="fbm-agent">{label(group.agent)}</div>
          {group.items.map((option) => {
            const isSelected = selected !== undefined
              && selected.agent === option.agent
              && (selected.model ?? null) === option.model
              && linkMode(selected, replaced) === option.mode;
            const inChain = !isSelected && taken.includes(option.agent) && selected?.agent !== option.agent;
            return (
              <button
                key={`${option.model ?? ''}|${option.mode}`}
                type="button"
                role="menuitem"
                className="fbm-item"
                disabled={inChain}
                aria-current={isSelected ? 'true' : undefined}
                title={inChain ? t('fallback.inChain') : undefined}
                data-testid={`${testId}-${option.agent}-${option.mode}${option.model ? `-${option.model}` : ''}`}
                onClick={() => onPick(option)}
              >
                <span className="fbm-model">{option.model ?? t('fallback.modelDefault')}</span>
                <span className="fbp-mode">{t(`fallback.mode.${option.mode}`)}</span>
                <TransportBadge transport={transportOf(option.agent)} />
              </button>
            );
          })}
        </div>
      ))}
    </div>
  );
}

interface RowProps {
  /** Prefix of the test ids. */
  testId: string;
  mode: FallbackMode;
  /** The agent the row belongs to: shown first, never allowed in the chain. `null` for a discussion-wide row. */
  origin: AgentType | null;
  /** The model this mode runs on today: the first pill of the row. */
  current?: { agent: AgentType; model: string | null } | null;
  /** The fallbacks set here, in order (one per row; an override may carry more). */
  links: AgentFallback[];
  /** What the backend resolved for them (gaps of each link, its own refusal). */
  view?: ChainView;
  /** The fallbacks reached through the rows of the fallbacks, dashed. */
  inherited?: InheritedLink[];
  /** Every agent/model that could be a fallback, any mode; the menu keeps the kind of this row's mode. */
  options: FallbackOption[];
  disabled?: boolean;
  /** Shown when nothing is set here. */
  empty?: ReactNode;
  /** Beside the add button: the override's « Back to default » / « Cut fallback ». */
  actions?: ReactNode;
  /** Applies the new list; a refusal (loop, depth, kind, access) rejects with its reason. */
  onChange: (next: AgentFallback[]) => Promise<void>;
  /** A click on an inherited pill opens the card of the agent that holds it. */
  onOpenAgent?: (agent: AgentType) => void;
}

/** One mode of an agent, or of a discussion or a step, as proposal B draws it:
 *  `[current model] → [fallback] → [inherited fallback, dashed] + Add a fallback`.
 *  Clicking a pill changes it, ✕ removes it; the menu is filtered by kind of
 *  mode, grouped by agent, and a model already in the chain is disabled. */
export function FallbackRow({
  testId, mode, origin, current, links, view, inherited = [], options, disabled, empty, actions, onChange, onOpenAgent,
}: RowProps) {
  const { t } = useT();
  const [menu, setMenu] = useState<null | { index: number }>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const apply = async (next: AgentFallback[]) => {
    const problem = chainProblem(origin, mode, next);
    if (problem) {
      setRefusal(t(problem.key, ...problem.args));
      return;
    }
    setBusy(true);
    try {
      await onChange(next);
      setRefusal(null);
    } catch (error) {
      setRefusal(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  const locked = disabled || busy;
  const offered = optionsFor(options, origin, mode);
  const taken: AgentType[] = [
    ...(origin ? [origin] : []),
    ...links.map((link) => link.agent),
    ...inherited.map((entry) => entry.link.agent),
  ];
  const pick = (index: number, option: FallbackOption) => {
    setMenu(null);
    const link = toLink(option, mode);
    void apply(index >= links.length ? [...links, link] : links.map((existing, at) => (at === index ? link : existing)));
  };
  const gapsOf = (index: number) => view?.links[index]?.gaps ?? [];
  const modeLabel = t(`fallback.mode.${mode}`);

  return (
    <div className="fbr" data-testid={testId} data-mode={mode} data-kind={modeKind(mode)}>
      <span className="fbr-mode">{modeLabel}</span>
      <div className="fbr-track">
        {current && (
          <FallbackPill
            testId={`${testId}-current`}
            tone="current"
            agent={current.agent}
            model={current.model}
            transport={transportOf(current.agent)}
            title={t('fallback.current', current.model ?? t('fallback.modelDefault'))}
          />
        )}
        {links.map((link, index) => (
          <span className="fbr-step" key={`${link.agent}-${link.model ?? ''}-${index}`}>
            <ArrowRight size={11} className="fbr-arrow" aria-hidden="true" />
            <span className="fbr-anchor">
              <FallbackPill
                testId={`${testId}-link-${index}`}
                tone="own"
                agent={link.agent}
                model={link.model}
                mode={link.mode && link.mode !== mode ? link.mode : undefined}
                transport={transportOf(link.agent)}
                gaps={gapsOf(index)}
                title={t('fallback.change')}
                disabled={locked}
                removeLabel={t('fallback.remove')}
                onClick={() => setMenu(menu?.index === index ? null : { index })}
                onRemove={() => void apply(links.filter((_, at) => at !== index))}
              />
              {menu?.index === index && (
                <FallbackMenu
                  testId={`${testId}-menu`}
                  options={offered}
                  taken={taken}
                  selected={link}
                  replaced={mode}
                  onPick={(option) => pick(index, option)}
                  onClose={() => setMenu(null)}
                />
              )}
            </span>
          </span>
        ))}
        {inherited.map((entry, index) => (
          <span className="fbr-step" key={`inherited-${entry.link.agent}-${index}`}>
            <ArrowRight size={11} className="fbr-arrow" aria-hidden="true" />
            <FallbackPill
              testId={`${testId}-inherited-${index}`}
              tone="inherited"
              agent={entry.link.agent}
              model={entry.link.model}
              mode={entry.link.mode !== mode ? entry.link.mode : undefined}
              transport={entry.link.transport}
              gaps={entry.link.gaps}
              title={`${t('fallback.setOn', label(entry.source))} — ${t('fallback.inheritedHint')}`}
              onClick={onOpenAgent ? () => onOpenAgent(entry.source) : undefined}
            />
          </span>
        ))}
        {links.length === 0 && empty !== undefined && empty}
        {links.length === 0 && empty === undefined && <span className="fbr-off">{t('fallback.off')}</span>}
        {links.length === 0 && (
          <span className="fbr-anchor">
            <button
              type="button"
              className="fbr-add"
              disabled={locked}
              data-testid={`${testId}-add`}
              onClick={() => setMenu(menu?.index === links.length ? null : { index: links.length })}
            >
              <Plus size={11} aria-hidden="true" />
              {t('fallback.add')}
            </button>
            {menu?.index === links.length && (
              <FallbackMenu
                testId={`${testId}-menu`}
                options={offered}
                taken={taken}
                replaced={mode}
                onPick={(option) => pick(links.length, option)}
                onClose={() => setMenu(null)}
              />
            )}
          </span>
        )}
        {actions}
      </div>
      {(view?.error || refusal) && (
        <div className="fbr-error" role="alert" data-testid={`${testId}-refused`}>
          <AlertTriangle size={11} aria-hidden="true" />
          {refusal ? t('fallback.refused', refusal) : view?.error}
        </div>
      )}
    </div>
  );
}

import {
  useEffect, useLayoutEffect, useRef, useState,
  type KeyboardEvent as ReactKeyboardEvent, type ReactNode,
} from 'react';
import { createPortal } from 'react-dom';
import { AlertTriangle, ArrowDown, ArrowUp, Plus, X } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { AGENT_LABELS } from '../../lib/constants';
import type { AgentFallback, AgentType, ChainView, FallbackGap, FallbackMode, FallbackTransport } from '../../types/generated';
import {
  MAX_FALLBACK_CHAIN,
  TRANSPORT_LABEL,
  chainProblem,
  linkMode,
  modeKind,
  optionsFor,
  placeMenu,
  sourceOf,
  toLink,
  transportOf,
  transportOfOption,
  type FallbackOption,
  type InheritedLink,
  type MenuPlacement,
} from '../../lib/fallbackChain';
import './FallbackRow.css';

const label = (agent: AgentType) => AGENT_LABELS[agent] ?? agent;

/** The warnings of a fallback, in the UI's language: the backend sends a stable code, never a sentence. */
const gapText = (t: (key: string, ...args: string[]) => string, gaps: FallbackGap[]) =>
  gaps.map((gap) => {
    const text = t(`fallback.gap.${gap}`);
    // A code this build does not know (a newer backend) is shown as it came rather than as a key.
    return text === `fallback.gap.${gap}` ? gap : text;
  }).join(', ');

/** The icons of the text modes, the same as the default-tier chooser's. */
const MODE_ICON: Partial<Record<FallbackMode, string>> = { eco: '⚡', standard: '🎯', advanced: '🧠' };

/** The label of a mode row, its icon first when the mode has one. */
export function FallbackModeLabel({ mode }: { mode: FallbackMode }) {
  const { t } = useT();
  const icon = MODE_ICON[mode];
  return (
    <span className="fbr-mode">
      {icon && <span className="fbr-mode-icon" aria-hidden="true">{icon}</span>}
      {t(`fallback.mode.${mode}`)}
    </span>
  );
}

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
  /** The name shown in place of the agent's: a media source that is a named connection. */
  name?: string;
  model: string | null | undefined;
  transport: FallbackTransport;
  /** « eco », « advanced »…: shown when the model serves another mode than its row. */
  mode?: FallbackMode;
  gaps?: FallbackGap[];
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
  testId, agent, name, model, transport, mode, gaps = [], tone, title, disabled, onClick, onRemove, removeLabel,
}: PillProps) {
  const { t } = useT();
  const body = (
    <>
      <span className="fbp-agent">{name ?? label(agent)}</span>
      <span className="fbp-model">{model ?? t('fallback.modelDefault')}</span>
      {mode && <span className="fbp-mode">{t(`fallback.mode.${mode}`)}</span>}
      <TransportBadge transport={transport} />
      {gaps.length > 0 && (
        <span className="fbp-gap" title={t('fallback.gaps', label(agent), gapText(t, gaps))}>
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

export interface MenuProps {
  testId: string;
  options: FallbackOption[];
  /** The element the menu hangs from: the menu is drawn in the page body, so no card clips it. */
  anchor: HTMLElement | null;
  /** Sources the walk already goes through: never offered again (a chain never has a source twice). */
  taken: string[];
  /** The target being changed, if any: marked, not disabled. */
  selected?: { agent: AgentType; connection?: string; model?: string | null; mode?: FallbackMode | null };
  replaced: FallbackMode;
  onPick: (option: FallbackOption) => void;
  onClose: () => void;
}

const keyOf = (option: FallbackOption) => `${sourceOf(option)}|${option.model ?? ''}|${option.mode}`;
const nameOf = (option: FallbackOption) => option.name ?? label(option.agent);

/** The options of a row grouped by agent, each with its model, its mode and its
 *  transport. A search field filters by agent and model; the arrows, Enter and
 *  Escape drive it from the keyboard. */
export function FallbackMenu({ testId, options, anchor, taken, selected, replaced, onPick, onClose }: MenuProps) {
  const { t } = useT();
  const ref = useRef<HTMLDivElement>(null);
  const [query, setQuery] = useState('');
  const [active, setActive] = useState(0);
  const [placement, setPlacement] = useState<MenuPlacement | null>(null);

  useEffect(() => {
    const away = (event: MouseEvent) => {
      const target = event.target as Node;
      // The anchor's own button toggles the menu: closing here would make its click reopen it.
      if (ref.current?.contains(target) || anchor?.contains(target)) return;
      onClose();
    };
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('mousedown', away);
      document.removeEventListener('keydown', escape);
    };
  }, [onClose, anchor]);

  const needle = query.trim().toLowerCase();
  const shown = needle
    ? options.filter((option) => `${nameOf(option)} ${option.agent} ${option.model ?? ''}`.toLowerCase().includes(needle))
    : options;
  const groups: { source: string; name: string; items: FallbackOption[] }[] = [];
  for (const option of shown) {
    const group = groups.find((entry) => entry.source === sourceOf(option));
    if (group) group.items.push(option);
    else groups.push({ source: sourceOf(option), name: nameOf(option), items: [option] });
  }

  const isSelected = (option: FallbackOption) => selected !== undefined
    && sourceOf(selected) === sourceOf(option)
    && (selected.model ?? null) === option.model
    && linkMode(selected, replaced) === option.mode;
  const isTaken = (option: FallbackOption) => !isSelected(option)
    && taken.includes(sourceOf(option)) && (selected ? sourceOf(selected) : undefined) !== sourceOf(option);
  // The keyboard walks the options that can be picked, in the order they are drawn.
  const pickable = groups.flatMap((group) => group.items).filter((option) => !isTaken(option));
  const current = Math.min(active, Math.max(pickable.length - 1, 0));
  const activeKey = pickable[current] ? keyOf(pickable[current]) : null;

  // Placed before paint, and again when the list (so its height) or the viewport changes.
  useLayoutEffect(() => {
    if (!anchor) return;
    const place = () => {
      if (!ref.current) return;
      const box = ref.current.getBoundingClientRect();
      setPlacement(placeMenu(
        anchor.getBoundingClientRect(),
        { width: box.width, height: box.height },
        { width: window.innerWidth, height: window.innerHeight },
      ));
    };
    place();
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    return () => {
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
    };
  }, [anchor, shown.length]);

  useEffect(() => {
    ref.current?.querySelector<HTMLElement>('[data-active="true"]')?.scrollIntoView?.({ block: 'nearest' });
  }, [activeKey]);

  const onKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (pickable.length === 0) return;
      setActive((current + (event.key === 'ArrowDown' ? 1 : pickable.length - 1)) % pickable.length);
    } else if (event.key === 'Home' || event.key === 'End') {
      event.preventDefault();
      setActive(event.key === 'Home' ? 0 : Math.max(pickable.length - 1, 0));
    } else if (event.key === 'Enter') {
      event.preventDefault();
      if (pickable[current]) onPick(pickable[current]);
    } else if (event.key === 'Escape') {
      event.stopPropagation();
      onClose();
    }
  };

  const menu = (
    <div
      className="fbm"
      aria-label={t('fallback.menu')}
      ref={ref}
      data-testid={testId}
      style={placement
        ? { left: placement.left, top: placement.top, maxHeight: placement.maxHeight }
        : { left: 0, top: 0, visibility: 'hidden' }}
    >
      <input
        type="search"
        className="fbm-search"
        autoFocus
        value={query}
        placeholder={t('fallback.search')}
        aria-label={t('fallback.search')}
        aria-activedescendant={activeKey ? `${testId}-opt-${activeKey}` : undefined}
        data-testid={`${testId}-search`}
        onChange={(event) => { setQuery(event.target.value); setActive(0); }}
        onKeyDown={onKeyDown}
      />
      <div className="fbm-list" role="menu">
        {options.length === 0 && <div className="fbm-empty">{t('fallback.menuEmpty')}</div>}
        {options.length > 0 && groups.length === 0 && (
          <div className="fbm-empty" data-testid={`${testId}-nomatch`}>{t('fallback.menuNoMatch')}</div>
        )}
        {groups.map((group) => (
          <div key={group.source} className="fbm-group" role="group" aria-label={group.name}>
            <div className="fbm-agent">{group.name}</div>
            {group.items.map((option) => {
              const inChain = isTaken(option);
              return (
                <button
                  key={`${option.model ?? ''}|${option.mode}`}
                  id={`${testId}-opt-${keyOf(option)}`}
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  className="fbm-item"
                  disabled={inChain}
                  aria-current={isSelected(option) ? 'true' : undefined}
                  data-active={activeKey === keyOf(option) ? 'true' : undefined}
                  title={inChain ? t('fallback.inChain') : undefined}
                  data-testid={`${testId}-${sourceOf(option)}-${option.mode}${option.model ? `-${option.model}` : ''}`}
                  onMouseMove={() => {
                    const at = pickable.findIndex((entry) => keyOf(entry) === keyOf(option));
                    if (at >= 0 && at !== current) setActive(at);
                  }}
                  onClick={() => onPick(option)}
                >
                  <span className="fbm-model">{option.model ?? t('fallback.modelDefault')}</span>
                  <span className="fbp-mode">{t(`fallback.mode.${option.mode}`)}</span>
                  <TransportBadge transport={transportOfOption(option)} />
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </div>
  );
  return createPortal(menu, document.body);
}

interface ItemProps {
  testId: string;
  /** 1-based place in the chain, in the order Kronn tries it. */
  position: number;
  /** `own`: stored on this row (editable); `inherited`: stored on another card's row (read-only here). */
  tone: 'own' | 'inherited';
  agent: AgentType;
  /** The name shown in place of the agent's: a media source that is a named connection. */
  name?: string;
  model: string | null | undefined;
  /** The tier tag (« advanced »): the mode the model runs in. */
  mode: FallbackMode;
  transport: FallbackTransport;
  /** What this fallback may lack (reachability / capabilities): drawn beside it, never on another one. */
  gaps?: FallbackGap[];
  title?: string;
  disabled?: boolean;
  /** Own: opens the change menu. */
  onClick?: () => void;
  /** Inherited only: the card that holds this fallback, named next to its number; a click opens it. */
  via?: { name: string; onOpen?: () => void };
  /** Own only, drawn right beside the chip: × always; ↑ ↓ only when this item has a neighbour to
   *  swap with (a control that could do nothing is not drawn); « Change » when there is no add button. */
  controls?: {
    onRemove: () => void;
    onChange?: () => void;
    up?: () => void;
    down?: () => void;
  };
  /** The change menu is anchored on the item's main button. */
  anchorRef?: (element: HTMLElement | null) => void;
  children?: ReactNode;
}

/** One numbered entry of a fallback chain: `1 · Codex · gpt-5.6 · standard · CLI ⚠ … ↑ ↓ ✕`.
 *  The same item on every card (CLI, Local, HTTP); an inherited one is dashed and has no controls. */
export function FallbackChainItem({
  testId, position, tone, agent, name, model, mode, transport, gaps = [], title, disabled, onClick, via, controls, anchorRef, children,
}: ItemProps) {
  const { t } = useT();
  const shown = name ?? label(agent);
  const body = (
    <>
      <span className="fbp-agent">{shown}</span>
      <span className="fbp-model">{model ?? t('fallback.modelDefault')}</span>
      <span className="fbp-mode">{t(`fallback.mode.${mode}`)}</span>
      <TransportBadge transport={transport} />
    </>
  );
  return (
    <li className="fbc" data-tone={tone} data-testid={testId}>
      <span className="fbc-n" aria-hidden="true" data-testid={`${testId}-position`}>{position}</span>
      {via && (via.onOpen ? (
        <button
          type="button"
          className="fbc-via fbc-via-link"
          title={t('fallback.openCard', via.name)}
          data-testid={`${testId}-via`}
          onClick={via.onOpen}
        >
          {t('fallback.via', via.name)}
        </button>
      ) : (
        <span className="fbc-via" title={t('fallback.setOn', via.name)} data-testid={`${testId}-via`}>{t('fallback.via', via.name)}</span>
      ))}
      <span className="fbc-anchor" ref={anchorRef}>
        {onClick ? (
          <button type="button" className="fbc-main" title={title} disabled={disabled} onClick={onClick}>{body}</button>
        ) : (
          <span className="fbc-main" title={title}>{body}</span>
        )}
        {controls?.onChange && (
          <button
            type="button"
            className="fbc-change"
            disabled={disabled}
            title={t('fallback.change')}
            data-testid={`${testId}-change`}
            onClick={controls.onChange}
          >
            {t('fallback.changeShort')}
          </button>
        )}
        {children}
      </span>
      {controls && (
        <span className="fbc-controls">
          {controls.up && (
            <button
              type="button"
              className="fbc-btn"
              disabled={disabled}
              title={t('fallback.moveUp')}
              aria-label={t('fallback.moveUp')}
              data-testid={`${testId}-up`}
              onClick={controls.up}
            >
              <ArrowUp size={11} aria-hidden="true" />
            </button>
          )}
          {controls.down && (
            <button
              type="button"
              className="fbc-btn"
              disabled={disabled}
              title={t('fallback.moveDown')}
              aria-label={t('fallback.moveDown')}
              data-testid={`${testId}-down`}
              onClick={controls.down}
            >
              <ArrowDown size={11} aria-hidden="true" />
            </button>
          )}
          <button
            type="button"
            className="fbc-btn fbc-remove"
            disabled={disabled}
            title={t('fallback.remove')}
            aria-label={t('fallback.remove')}
            data-testid={`${testId}-remove`}
            onClick={controls.onRemove}
          >
            <X size={11} aria-hidden="true" />
          </button>
        </span>
      )}
      {gaps.length > 0 && (
        <span className="fbp-gap" role="note" title={t('fallback.gaps', shown, gapText(t, gaps))} data-testid={`${testId}-warning`}>
          <AlertTriangle size={11} aria-hidden="true" />
          {gapText(t, gaps)}
        </span>
      )}
    </li>
  );
}

interface RowProps {
  /** Prefix of the test ids. */
  testId: string;
  mode: FallbackMode;
  /** The agent the row belongs to: shown first, never allowed in the chain. `null` for a discussion-wide row. */
  origin: AgentType | null;
  /** The model this mode runs on today: shown first when the row has no model picker. */
  current?: { agent: AgentType; model: string | null } | null;
  /** KT-1706 — the model picker of the mode: when given it IS the first element of the row's
   *  right-hand column, in place of the read-only `current` pill. */
  model?: ReactNode;
  /** Another setting of the mode, on the line of its model picker (an agent's reasoning effort).
   *  A row with no picker has no such line and does not draw it. */
  extra?: ReactNode;
  /** The fallbacks set here, in order (an agent row stores one; an override may carry more). */
  links: AgentFallback[];
  /** What the backend resolved for them (gaps of each link, its own refusal). */
  view?: ChainView;
  /** The fallbacks reached through the rows of the fallbacks: listed after the own ones, read-only. */
  inherited?: InheritedLink[];
  /** Every agent/model that could be a fallback, any mode; the menu keeps the kind of this row's mode. */
  options: FallbackOption[];
  /** How many fallbacks this row can store itself (the engine keeps one per agent mode row). */
  maxLinks?: number;
  disabled?: boolean;
  /** Shown when nothing is set here. */
  empty?: ReactNode;
  /** Beside the add button: the override's « Back to default » / « Cut fallback ». */
  actions?: ReactNode;
  /** Applies the new list; a refusal (loop, depth, kind, access) rejects with its reason. */
  onChange: (next: AgentFallback[]) => Promise<void>;
  /** A click on an inherited item opens the card of the agent that holds it. */
  onOpenAgent?: (agent: AgentType) => void;
}

/** One mode of an agent, or of a discussion or a step, as proposal A draws it:
 *  the label on the left, and on the right ONE column — the model, « If
 *  unavailable: », then the fallbacks numbered in the order Kronn tries them,
 *  then « + Add a fallback ». The links stored here come first (↑ ↓ ✕ reorder,
 *  remove, change them); those reached through the rows of the fallbacks follow,
 *  dashed and read-only (they live on another card). The menu is filtered by
 *  kind of mode, grouped by agent, and a model already in the chain is disabled. */
export function FallbackRow({
  testId, mode, origin, current, model, extra, links, view, inherited = [], options, maxLinks = MAX_FALLBACK_CHAIN, disabled, empty, actions, onChange, onOpenAgent,
}: RowProps) {
  const { t } = useT();
  const [menu, setMenu] = useState<null | { index: number }>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The anchor of the open menu; the menu is drawn in the page body and placed from it.
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const closeMenu = () => setMenu(null);

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
  const move = (index: number, by: -1 | 1) => {
    const next = [...links];
    [next[index], next[index + by]] = [next[index + by], next[index]];
    void apply(next);
  };
  const gapsOf = (index: number) => view?.links[index]?.gaps ?? [];
  const anchorFor = (index: number) => (element: HTMLElement | null) => {
    if (menu?.index === index && element) setAnchor((previous) => (previous === element ? previous : element));
  };
  const full = links.length >= maxLinks;
  const nothing = links.length === 0 && inherited.length === 0;
  const picker = model !== undefined && <span className="fbr-model" data-testid={`${testId}-model`}>{model}</span>;

  return (
    <div
      className="fbr"
      data-testid={testId}
      data-mode={mode}
      data-kind={modeKind(mode)}
      data-with-model={model !== undefined ? 'true' : undefined}
    >
      <FallbackModeLabel mode={mode} />
      <div className="fbr-track">
        {picker && extra ? (
          <div className="fbr-line">
            {picker}
            <span className="fbr-extra">{extra}</span>
          </div>
        ) : picker}
        {model === undefined && current && (
          <FallbackPill
            testId={`${testId}-current`}
            tone="current"
            agent={current.agent}
            model={current.model}
            transport={transportOf(current.agent)}
            title={t('fallback.current', current.model ?? t('fallback.modelDefault'))}
          />
        )}
        <span className="fbr-caption">{t('fallback.ifUnavailable')}</span>
        {!nothing && (
          <ol className="fbc-list" data-testid={`${testId}-chain`}>
            {links.map((link, index) => (
              <FallbackChainItem
                key={`${link.agent}-${link.model ?? ''}-${index}`}
                testId={`${testId}-link-${index}`}
                position={index + 1}
                tone="own"
                agent={link.agent}
                model={link.model}
                mode={linkMode(link, mode)}
                transport={transportOf(link.agent)}
                gaps={gapsOf(index)}
                title={t('fallback.change')}
                disabled={locked}
                onClick={() => setMenu(menu?.index === index ? null : { index })}
                anchorRef={anchorFor(index)}
                controls={{
                  onRemove: () => void apply(links.filter((_, at) => at !== index)),
                  // Only a row that stores several links can reorder them, and only toward a neighbour.
                  onChange: full && links.length === 1 ? () => setMenu(menu?.index === index ? null : { index }) : undefined,
                  up: index > 0 ? () => move(index, -1) : undefined,
                  down: index < links.length - 1 ? () => move(index, 1) : undefined,
                }}
              >
                {menu?.index === index && (
                  <FallbackMenu
                    testId={`${testId}-menu`}
                    options={offered}
                    anchor={anchor}
                    taken={taken}
                    selected={link}
                    replaced={mode}
                    onPick={(option) => pick(index, option)}
                    onClose={closeMenu}
                  />
                )}
              </FallbackChainItem>
            ))}
            {inherited.map((entry, index) => (
              <FallbackChainItem
                key={`inherited-${entry.link.agent}-${index}`}
                testId={`${testId}-inherited-${index}`}
                position={links.length + index + 1}
                tone="inherited"
                agent={entry.link.agent}
                model={entry.link.model}
                mode={entry.link.mode}
                transport={entry.link.transport}
                gaps={entry.link.gaps}
                title={t('fallback.inheritedHint')}
                via={{ name: label(entry.source), onOpen: onOpenAgent ? () => onOpenAgent(entry.source) : undefined }}
              />
            ))}
          </ol>
        )}
        {nothing && empty !== undefined && empty}
        {nothing && empty === undefined && <span className="fbr-off" data-testid={`${testId}-none`}>{t('fallback.none')}</span>}
        {(!full || actions) && (
          <div className="fbr-foot">
            {!full && (
              <span className="fbr-anchor" ref={anchorFor(links.length)}>
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
                    anchor={anchor}
                    taken={taken}
                    replaced={mode}
                    onPick={(option) => pick(links.length, option)}
                    onClose={closeMenu}
                  />
                )}
              </span>
            )}
            {actions}
          </div>
        )}
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

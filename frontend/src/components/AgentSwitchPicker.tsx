import {
  Fragment, useCallback, useEffect, useId, useImperativeHandle, useLayoutEffect, useRef, useState,
  type CSSProperties, type FocusEvent as ReactFocusEvent, type KeyboardEvent as ReactKeyboardEvent, type ReactNode, type Ref, type RefObject,
} from 'react';
import { agentPaletteOptionId } from '../lib/agentPickerPalette';
import { createPortal } from 'react-dom';
import { Check, ChevronDown, ChevronRight, Loader2, RefreshCw } from 'lucide-react';
import {
  resolveCatalogTier, matchesCatalogSearch, catalogTargetSearchTerms, modelRuntimeTargetId, modeModelId,
  type ResolvedCatalogTier,
} from '../lib/modelCatalogSelection';
import { useModelCatalogSnapshot } from '../hooks/useModelCatalogSnapshot';
import { isFreeModel } from '../lib/freeModel';
import { useT } from '../lib/I18nContext';
import {
  readAgentPickerRecents, recordAgentPickerRecent, recentKey, type AgentPickerRecent,
} from '../lib/agentPickerRecents';
import { userError } from '../lib/userError';
import {
  AGENT_COLORS,
  AGENT_LABELS,
  ALL_AGENT_TYPES,
  MODEL_TIER_ICONS,
  agentColor,
  agentTextColor,
} from '../lib/constants';
import { agentTransport, type AgentTransport } from '../lib/shadowMerge';
import {
  availabilityBlocks, rowAvailability, sameAvailability, sharedTierState, tierAvailability, tierChoiceState,
  type AgentAvailability, type AgentAvailabilityState,
} from '../lib/agentAvailability';
import { SearchField } from './ui/SearchField';
import { AgentIdentity } from './ui/AgentIdentity';
import { agentIdentityText, type AgentIdentityModel } from './ui/agentIdentityKinds';
import { resolveIdentityModel } from '../lib/agentModelResolution';
import { agentModelLabel } from './workflows/shadowRowModel';
import type {
  AgentType,
  CatalogModelEntry,
  ModelCatalogSnapshot,
  ModelTier,
  ModelTierConfig,
  ModelTiersConfig,
  ShadowAgent,
} from '../types/generated';
import './AgentSwitchPicker.css';

type Translate = (key: string, ...args: (string | number)[]) => string;
type TranslatePlural = (key: string, count: number, ...args: (string | number)[]) => string;

/**
 * KT-2010 — « Gratuit » / "Free" after the id of a free model (`isFreeModel`),
 * on every row of the list that shows one. The row's accessible name carries
 * the same word (`withFreeName`). `displayName` is the catalogue entry's,
 * when the row has one: a model can be free by its display name only.
 */
function FreeModelBadge({ modelId, displayName, t }: {
  modelId: string | null | undefined; displayName?: string | null; t: Translate;
}) {
  if (!isFreeModel(modelId, displayName)) return null;
  return <span className="kr-agent-switch-free-badge" data-testid="agent-switch-free-badge">{t('agentPicker.freeBadge')}</span>;
}

type MaybeText = string | null | undefined;

/** A row's accessible name, « … · Gratuit » when its model is free. */
const withFreeName = (name: string, modelId: MaybeText, displayName: MaybeText, t: Translate) =>
  isFreeModel(modelId, displayName) ? `${name} · ${t('agentPicker.freeBadge')}` : name;

/** What the search finds a free model by: the badge's word, and `free` in every language. */
const freeSearchTerms = (modelId: MaybeText, displayName: MaybeText, t: Translate): string[] =>
  isFreeModel(modelId, displayName) ? [t('agentPicker.freeBadge'), 'free'] : [];

/**
 * KT-1994 — the most other models an unfolded agent mounts at once (577 for
 * OpenRouter on 2026-11-21): the rest is counted, and the search reaches it.
 */
export const OTHER_MODELS_RENDER_LIMIT = 50;

/**
 * KT-1994 — a model id that breaks only after `/` and `:` (a <wbr> there),
 * never inside a token: each part is one unbroken piece (a hyphen is no
 * break either).
 */
function breakableId(id: string): ReactNode {
  const parts = id.split(/(?<=[/:])/);
  return parts.map((part, index) => (
    <Fragment key={index}>
      <span className="kr-agent-switch-id-part">{part}</span>
      {index < parts.length - 1 && <wbr />}
    </Fragment>
  ));
}

/** What a tier choice says of itself: its tooltip, its description and its states. */
function tierDetails(
  tier: ModelTier | null,
  resolution: ResolvedCatalogTier | null,
  name: string,
  t: Translate,
) {
  const availability = tierAvailability(resolution, t);
  const state = tier === null ? null : tierChoiceState(resolution, t);
  const title = [
    name,
    state && state.state !== 'not-listed' ? state.label : '',
    resolution?.notice,
    resolution?.refusal,
  ].filter(Boolean).join(' — ');
  const provenance = resolution?.entry && resolution.provenance
    ? t(`modelCatalog.provenance.${resolution.provenance}`)
    : resolution?.configured ? t('modelCatalog.notInCatalog') : '';
  const description = [title, resolution?.entry?.model_id, provenance,
    availability?.state === 'unavailable' ? availability.label : '',
    resolution?.refusal ?? resolution?.entry?.unavailable_reason ?? '',
    resolution?.replacement
      ? t('modelCatalog.runsInstead', resolution.replacement, resolution.configured || resolution.entry?.model_id || '')
      : '',
    resolution?.notice ?? '',
    resolution?.entry ? t('modelCatalog.lastChecked', resolution.entry.last_checked_at) : '',
  ].filter(Boolean).join(' · ');
  return { availability, state, title, description, blocked: availabilityBlocks(availability) };
}

export interface AgentSwitchTarget {
  agent: AgentType;
  connectionId?: string | null;
  label?: string;
  modelTiers?: ModelTierConfig;
}

// ─── Availability: one badge, the same everywhere (KT-1885) ──────────────

export type { AgentAvailability, AgentAvailabilityState };

const AVAILABILITY_TEST_IDS: Record<AgentAvailabilityState, string> = {
  'needs-full-access': 'agent-needs-full-access',
  unavailable: 'agent-unavailable',
  disabled: 'agent-disabled',
  'not-listed': 'agent-not-listed',
  catalog: 'agent-catalog-state',
};

export function AgentAvailabilityBadge({ availability, placement = 'inline' }: {
  availability?: AgentAvailability | null;
  /** `tier`: under a tier choice's label, inside its button. */
  placement?: 'inline' | 'tier';
}) {
  if (!availability) return null;
  return (
    <span
      className={placement === 'tier' ? 'kr-agent-switch-catalog-meta' : 'kr-agent-switch-badge'}
      data-state={availability.state}
      data-testid={AVAILABILITY_TEST_IDS[availability.state]}
      title={availability.title}
    >
      {availability.label}
    </span>
  );
}

// ─── Tier choices: the picker's and the mention menu's ───────────────────

export interface AgentPickerTierChoicesProps {
  /** Who the tiers belong to, first in each choice's name (`Claude Code · Reasoning`). */
  targetLabel: string;
  /** `null` is « the agent's own default », offered when the caller can go back to it. */
  tiers: readonly (ModelTier | null)[];
  currentTier?: ModelTier | null;
  /** The tier the keyboard points at, in a list driven from a text field. */
  keyboardTier?: ModelTier | null;
  resolve: (tier: ModelTier) => ResolvedCatalogTier;
  tierLabel: (tier: ModelTier) => string;
  /** The model named for a tier when the catalogue resolves none. */
  modelLabel: (tier: ModelTier) => string;
  /** How a tier is named in its title and accessible name (default `Tier · model`). */
  titleFor?: (tier: ModelTier) => string;
  defaultLabel: string;
  onSelect: (tier: ModelTier | null) => void;
  disabled?: boolean;
  /** The current choice cannot be chosen again (the popover's). */
  disableCurrent?: boolean;
  /**
   * The pointer chooses on press and keeps the focus where it is: the mention
   * menu's text field keeps its caret.
   */
  keepFocus?: boolean;
  itemRole?: 'menuitem';
  ariaLabel?: string;
  /**
   * KT-1994 — write the model id each tier runs under its name (the @ menu's
   * rows), « agent default model » when the tier declares none.
   */
  showModel?: boolean;
  t: Translate;
}

/**
 * KT-1885 — the state a row's tiers share, written once next to the agent
 * (`rowAvailability`); `AgentPickerTierChoices` derives the same one, so a
 * tier only carries a state that differs from it.
 */
function tierRowState(
  tiers: readonly (ModelTier | null)[],
  resolve: (tier: ModelTier) => ResolvedCatalogTier,
  t: Translate,
): AgentAvailability | null {
  return sharedTierState(tiers.flatMap(tier => tier === null ? [] : [tierChoiceState(resolve(tier), t)]));
}

export function AgentPickerTierChoices({
  targetLabel, tiers, currentTier, keyboardTier, resolve, tierLabel, modelLabel, titleFor, defaultLabel, onSelect,
  disabled = false, disableCurrent = false, keepFocus = false, itemRole, ariaLabel, showModel = false, t,
}: AgentPickerTierChoicesProps) {
  const id = useId();
  const shared = tierRowState(tiers, resolve, t);
  return (
    <span
      className="kr-agent-switch-tier-choices"
      aria-label={ariaLabel}
      data-models={showModel || undefined}
      style={{ '--agent-switch-tier-count': tiers.length } as CSSProperties}
    >
      {tiers.map(tier => {
        const selected = tier === (currentTier ?? null) && (tier !== null || currentTier === null);
        const label = tier === null ? defaultLabel : tierLabel(tier);
        // « Modèle par défaut de l'agent » stays the name; the button shows « Défaut ».
        const visibleLabel = tier === null ? t('agentPicker.defaultTier') : label;
        const resolution = tier === null ? null : resolve(tier);
        const name = tier === null ? label : titleFor?.(tier) ?? `${label} · ${modelLabel(tier)}`;
        const { state, title, description, blocked } = tierDetails(tier, resolution, name, t);
        // Written once on the row when the tiers share it; here only when this tier differs.
        const ownState = state && !sameAvailability(state, shared) ? state : null;
        const writtenModel = showModel && tier !== null ? modeModelId(resolution, tier) : '';
        // KT-1994 — the cell writes the id on two lines at most; its title and
        // accessible name keep it whole (the full sentence for a mode without one).
        const fullModel = showModel && tier !== null ? writtenModel || t('wf.shadow.rows.defaultModel') : '';
        const accessibleName = `${targetLabel} · ${titleFor && tier !== null ? name : label}`;
        const descriptionId = `${id}-${tier ?? 'agent-default'}`;
        const choose = () => { if (!blocked) onSelect(tier); };
        return (
          <button
            key={tier ?? 'agent-default'}
            type="button"
            role={itemRole}
            className="kr-agent-switch-tier-option"
            data-tier={tier ?? 'agent-default'}
            data-current={selected}
            data-keyboard-selected={keyboardTier !== undefined && keyboardTier === tier ? 'true' : undefined}
            aria-label={fullModel && !accessibleName.includes(fullModel) ? `${accessibleName} · ${fullModel}` : accessibleName}
            aria-describedby={descriptionId}
            title={fullModel && !title.includes(fullModel) ? `${title} · ${fullModel}` : title}
            disabled={disabled || (disableCurrent && selected) || blocked}
            // The text field driving the list keeps the keyboard: no Tab stop here.
            tabIndex={keepFocus ? -1 : undefined}
            onMouseDown={keepFocus ? event => {
              event.preventDefault();
              event.stopPropagation();
              choose();
            } : undefined}
            onClick={event => {
              // With keepFocus, a pointer already chose on press: only keyboard activation remains.
              if (keepFocus && event.detail !== 0) return;
              choose();
            }}
          >
            <span className="kr-agent-switch-tier-label">
              {tier !== null && <span aria-hidden="true">{MODEL_TIER_ICONS[tier]}</span>}
              <span>{visibleLabel}</span>
            </span>
            {showModel && tier !== null && (writtenModel
              ? <code className="kr-agent-switch-tier-model">{breakableId(writtenModel)}</code>
              : <span className="kr-agent-switch-tier-model" data-default="true">{t('agentPicker.defaultShort')}</span>)}
            <span id={descriptionId} hidden>{description}</span>
            {ownState && <AgentAvailabilityBadge availability={ownState} placement="tier" />}
            {selected && <Check size={8} aria-hidden="true" />}
          </button>
        );
      })}
    </span>
  );
}

// ─── Palette: the list a text field drives (the composer's @ menu) ───────

export interface AgentPickerPaletteRow {
  key: string;
  /** Colours the dot and the label; none for a row that is no agent (@all). */
  agent?: AgentType | null;
  label: ReactNode;
  detail?: ReactNode;
  /** A heading shown before the first row of each group. */
  group?: string;
  availability?: AgentAvailability | null;
  /** An icon after the label (a CLI session that manages its own model). */
  trailing?: ReactNode;
  highlighted?: boolean;
  onHover?: () => void;
  /** Choose the row as it stands; absent for a row that is only shown. */
  onSelect?: () => void;
  tiers?: Omit<AgentPickerTierChoicesProps, 'keepFocus' | 't' | 'tiers'> & { tiers?: readonly (ModelTier | null)[] };
}

/**
 * KT-1885 — the agent picker's list without its own trigger and search, for
 * a menu a text field drives: the composer types the query after « @ » and
 * moves the highlight with its own keys; the rows, the tier choices and the
 * availability badges are the picker's.
 */
export function AgentPickerPalette({ id, rows, status, ariaLabel, className, style, placement, listRef, boundary, t }: {
  /**
   * The list's id: its rows are `agentPaletteOptionId(id, row.key)`, which the
   * text field names in `aria-activedescendant` (see `agentPaletteFieldAria`).
   */
  id?: string;
  rows: readonly AgentPickerPaletteRow[];
  status?: ReactNode;
  ariaLabel: string;
  className?: string;
  style?: CSSProperties;
  /** Which side of its field the list opens on, for the caller's CSS. */
  placement?: 'above' | 'below';
  listRef?: Ref<HTMLDivElement>;
  /**
   * The area the list must stay in (the message list above the composer):
   * its height is measured against that area's edge on its open side, minus
   * `--kr-space-2`, and it scrolls inside. Without one, the window bounds it.
   */
  boundary?: () => Element | null | undefined;
  t: Translate;
}) {
  const fallbackId = useId();
  const listId = id ?? fallbackId;
  const ownRef = useRef<HTMLDivElement | null>(null);
  // The caller's ref gets the list's node through React, never written by hand.
  useImperativeHandle(listRef, () => ownRef.current as HTMLDivElement, []);
  const highlightedKey = rows.find(row => row.highlighted)?.key;
  // The keys move the highlight past the list's fold: keep it in view.
  useEffect(() => {
    if (!highlightedKey) return;
    const row = ownRef.current?.querySelector<HTMLElement>('[data-highlighted="true"]');
    row?.scrollIntoView?.({ block: 'nearest' });
  }, [highlightedKey]);
  // Never taller than the room it really has: from its anchored edge to the
  // boundary's far edge (or the window's), so its first row is always visible.
  // The latest `boundary`, updated after each commit and before `fit` runs
  // below (layout effects run in order), never written during render.
  const boundaryRef = useRef(boundary);
  useLayoutEffect(() => {
    boundaryRef.current = boundary;
  });
  const fit = useCallback(() => {
    const node = ownRef.current;
    if (!node || !boundaryRef.current) return;
    const own = node.getBoundingClientRect();
    const area = boundaryRef.current()?.getBoundingClientRect();
    const room = placement === 'below'
      ? Math.min(area?.bottom ?? window.innerHeight, window.innerHeight) - own.top - VIEWPORT_PADDING
      : own.bottom - Math.max(area?.top ?? 0, 0) - VIEWPORT_PADDING;
    node.style.maxHeight = `${Math.max(0, Math.floor(room))}px`;
  }, [placement]);
  // On every render: the query, the rows and the field's own height move it.
  useLayoutEffect(() => {
    fit();
  });
  const bounded = Boolean(boundary);
  useEffect(() => {
    if (!bounded) return;
    window.addEventListener('resize', fit);
    return () => window.removeEventListener('resize', fit);
  }, [fit, bounded]);
  return (
    <div
      ref={ownRef}
      id={listId}
      className={['kr-agent-switch-palette', className].filter(Boolean).join(' ')}
      role="listbox"
      aria-label={ariaLabel}
      style={style}
      data-placement={placement}
    >
      {status &&<div role="status" className="kr-agent-switch-status">{status}</div>}
      {rows.map((row, index) => {
        const blocked = availabilityBlocks(row.availability);
        const badge = rowAvailability(
          row.availability,
          row.tiers ? tierRowState(row.tiers.tiers ?? TIER_CHOICES, row.tiers.resolve, t) : null,
        );
        return (
          <Fragment key={row.key}>
            {row.group && row.group !== rows[index - 1]?.group && (
              <div className="kr-agent-switch-group" role="presentation">{row.group}</div>
            )}
            <div
              id={agentPaletteOptionId(listId, row.key)}
              className="kr-agent-switch-palette-row"
              role="option"
              aria-selected={Boolean(row.highlighted)}
              aria-disabled={blocked || !row.onSelect || undefined}
              data-highlighted={Boolean(row.highlighted)}
              onMouseEnter={row.onHover}
              onMouseDown={event => {
                // A tier choice chooses itself; never turn a refused one into the row's default.
                if (event.target instanceof Element && event.target.closest('.kr-agent-switch-tier-option')) return;
                // The text field keeps its focus and caret.
                event.preventDefault();
                if (!blocked) row.onSelect?.();
              }}
            >
              <button
                type="button"
                className="kr-agent-switch-palette-main"
                disabled={blocked || !row.onSelect}
                tabIndex={-1}
              >
                <span
                  className="kr-agent-switch-option-dot"
                  style={{ background: row.agent ? AGENT_COLORS[row.agent] ?? 'var(--kr-text-faint)' : 'var(--kr-accent)' }}
                />
                <span className="kr-agent-switch-palette-label" style={row.agent ? { color: agentTextColor(row.agent) } : undefined}>
                  {row.label}
                </span>
                {row.detail && <span className="kr-agent-switch-palette-detail">{row.detail}</span>}
                <AgentAvailabilityBadge availability={badge} />
                {row.trailing}
              </button>
              {row.tiers && (
                <AgentPickerTierChoices
                  {...row.tiers}
                  tiers={row.tiers.tiers ?? TIER_CHOICES}
                  keepFocus
                  showModel
                  t={t}
                />
              )}
            </div>
          </Fragment>
        );
      })}
    </div>
  );
}

// ─── Exact model: an agent and one of its model ids ──────────────────────

/** The agent a step runs on, for what the exact-model mode greys out. */
export interface MainAgent {
  agent: AgentType;
  connectionId?: string | null;
  model?: string | null;
  /** The step's tier: it picks the model configured when none is explicit. */
  tier?: ModelTier | null;
}

/** CLI / HTTP / Local, the badge the agent lists carry. */
export function TransportBadge({ agent, connectionId, t }: { agent: string; connectionId?: string | null; t: Translate }) {
  const transport: AgentTransport = agentTransport(agent, connectionId);
  return <span className="wf-shadow-transport" data-transport={transport}>{t(`wf.shadow.rows.transport.${transport}`)}</span>;
}

export interface ExactModelSelection {
  /** `undefined` is « = principal »: the step's own agent and model. */
  value: ShadowAgent | undefined;
  onChange: (next: ShadowAgent | undefined) => void;
  main: MainAgent;
  /** Trigger's test id; the list is `${testId}-menu`, « = principal » `${testId}-same`. */
  testId: string;
  /** The trigger's content; the current agent and model by default. */
  children?: ReactNode;
  className?: string;
}

// ─── The picker ──────────────────────────────────────────────────────────

interface AgentSwitchPickerProps {
  currentAgent: AgentType;
  availableAgents: AgentType[];
  currentConnectionId?: string | null;
  currentTargetLabel?: string;
  availableTargets?: AgentSwitchTarget[];
  onChange?: (agent: AgentType) => Promise<void>;
  currentTier?: ModelTier | null;
  /** Saved per-discussion/QP override; alternative selections resolve their own tiers. */
  currentModel?: string | null;
  onSelectionChange?: (agent: AgentType, tier: ModelTier) => Promise<void>;
  onTargetSelectionChange?: (target: AgentSwitchTarget, tier: ModelTier) => Promise<void>;
  /** Optional, explicit return to the caller's agent default (no tier override). */
  onDefaultTierSelection?: (target: AgentSwitchTarget) => Promise<void>;
  tierLabels?: Record<ModelTier, string>;
  modelTiers?: ModelTiersConfig | null;
  defaultModelLabel?: string;
  disabled?: boolean;
  /** Shorthand for `variant="compact"`. */
  compact?: boolean;
  /**
   * KT-1885 — the trigger: `chip` (default, a bordered chip in a line of text),
   * `compact` (no border, in a dense row) or `field` (full width, a form field).
   */
  variant?: 'chip' | 'compact' | 'field';
  title: string;
  ariaLabel: string;
  staticClassName?: string;
  /** The model shown after the agent; defaults to the current tier's resolved model. */
  suffix?: string;
  /**
   * The agent's role, shown as its own label next to the name. A role is
   * never shown alone: the current model stays visible with it, in the
   * switchable and in the static form.
   */
  role?: string;
  /** Model named for the current selection when nothing resolves one (instead of `defaultModelLabel`). */
  currentModelFallback?: string;
  displayName?: string;
  /** Agents that cannot run until their full access is turned on (badged in the list). */
  needsFullAccess?: (agent: AgentType) => boolean;
  /** Why a target cannot be chosen, or runs with a warning: badged, and disabled when it blocks. */
  availability?: (target: AgentSwitchTarget) => AgentAvailability | null | undefined;
  /**
   * KT-1885 — multi-choice: several targets at once. The list stays open and
   * each target toggles; `currentAgent` only names the trigger when nothing is chosen.
   */
  multiple?: {
    selected: readonly AgentSwitchTarget[];
    onToggle: (target: AgentSwitchTarget) => void;
    /** The trigger's text, e.g. « 3 agents ». */
    summary?: (selected: readonly AgentSwitchTarget[]) => string;
  };
  /**
   * KT-1885 — exact model: the choice is an agent and one of its model ids
   * (the catalogue's, or one typed in the search), or « = principal ». The
   * principal's own model is greyed out: « = principal » already says it.
   */
  exactModel?: ExactModelSelection;
  /**
   * KT-1994 — the full list (discussion header, new-discussion form): under
   * each agent's modes its other models, folded, and a typed id « as is »;
   * a « Récents » section on top. A choice outside the modes is an exact model
   * (« Personnalisé »), saved through `onSelect`; a refusal it throws is shown
   * next to the entry and the previous choice stays.
   */
  customModel?: CustomModelSelection;
}

export interface CustomModelSelection {
  /** The exact model held now; empty when the mode decides. */
  model?: string | null;
  onSelect: (target: AgentSwitchTarget, model: string) => Promise<void>;
}

const TIER_CHOICES: readonly ModelTier[] = ['economy', 'default', 'reasoning'];
const targetKey = (target: Pick<AgentSwitchTarget, 'agent' | 'connectionId'>) => `${target.agent}:${target.connectionId ?? ''}`;
const defaultTargetLabel = (target: AgentSwitchTarget) => target.label ?? AGENT_LABELS[target.agent] ?? target.agent;

/** Arrows, Home and End between the enabled items; Escape closes. */
function usePopoverKeys(
  popoverRef: RefObject<HTMLElement | null>,
  searchRef: RefObject<HTMLInputElement | null>,
  close: () => void,
) {
  return (event: ReactKeyboardEvent<HTMLElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      close();
      return;
    }
    const options = Array.from(popoverRef.current?.querySelectorAll<HTMLButtonElement>(
      '[role="menuitem"]:not(:disabled), [role="menuitemcheckbox"]:not(:disabled)',
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
}

/** Between the trigger and its popover (`--kr-space-1`). */
const POPOVER_GAP = 4;
/** The popover never comes closer to the window's edge (`--kr-space-2`). */
const VIEWPORT_PADDING = 8;

/**
 * Open state, fixed position under (or above) the trigger, outside click and Escape.
 *
 * KT-1885 — the popover is placed from its rendered size, measured on open
 * and again whenever `layoutKey` changes (search, catalogue load), when it
 * resizes and when the window does: it stays inside the window and never
 * covers its trigger. Opened above, its bottom is anchored to the trigger.
 */
function usePopover(layoutKey: unknown, busy = false) {
  const [open, setOpen] = useState(false);
  const restoreFocusRef = useRef(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const popoverRef = useRef<HTMLSpanElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const place = useCallback(() => {
    const trigger = rootRef.current?.getBoundingClientRect();
    const popover = popoverRef.current;
    if (!trigger || !popover) return;
    const viewportWidth = window.innerWidth;
    const viewportHeight = window.innerHeight;
    const box = popover.getBoundingClientRect();
    // Its height without the cap it may already carry: the list's hidden overflow added back.
    const results = popover.querySelector<HTMLElement>('.kr-agent-switch-results');
    const naturalHeight = box.height + (results ? Math.max(0, results.scrollHeight - results.clientHeight) : 0);
    const width = Math.min(box.width, viewportWidth - 2 * VIEWPORT_PADDING);
    const roomBelow = viewportHeight - trigger.bottom - POPOVER_GAP - VIEWPORT_PADDING;
    const roomAbove = trigger.top - POPOVER_GAP - VIEWPORT_PADDING;
    const below = naturalHeight <= roomBelow || roomBelow >= roomAbove;
    const left = Math.max(VIEWPORT_PADDING, Math.min(trigger.left, viewportWidth - width - VIEWPORT_PADDING));
    popover.style.left = `${Math.round(left)}px`;
    popover.style.top = below ? `${Math.round(trigger.bottom + POPOVER_GAP)}px` : '';
    popover.style.bottom = below ? '' : `${Math.round(viewportHeight - trigger.top + POPOVER_GAP)}px`;
    popover.style.maxHeight = `${Math.max(0, Math.floor(below ? roomBelow : roomAbove))}px`;
    popover.dataset.placement = below ? 'below' : 'above';
  }, []);

  // What `close` reads: kept in step with the state once it is committed
  // (never written during render), and set at once by `close` itself.
  const openRef = useRef(open);
  useLayoutEffect(() => {
    openRef.current = open;
  }, [open]);
  const close = useCallback((restoreFocus = true) => {
    // Already closed (an outside click while a choice was saving): the focus
    // stays where the user put it, no return to the trigger is scheduled.
    if (!openRef.current) return;
    restoreFocusRef.current = restoreFocus;
    openRef.current = false;
    setOpen(false);
  }, []);

  // Before the first paint, and again when what it shows changes.
  useLayoutEffect(() => {
    if (open) place();
  }, [open, place, layoutKey]);

  useEffect(() => {
    const popover = popoverRef.current;
    if (!open || !popover || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => place());
    observer.observe(popover);
    return () => observer.disconnect();
  }, [open, place]);

  // Back on the trigger once it is enabled again (a save disables it).
  useEffect(() => {
    if (!open && !busy && restoreFocusRef.current) {
      restoreFocusRef.current = false;
      rootRef.current?.querySelector('button')?.focus();
    }
  }, [open, busy]);

  useEffect(() => {
    if (!open) return;
    // The search field takes the focus, even when the picker sits in a dialog
    // (the popover is in <body>, above every overlay layer).
    searchRef.current?.focus();
    const closeOutside = (event: MouseEvent) => {
      const target = event.target as Node;
      // The focus stays where the click put it: no later return to the trigger.
      if (!rootRef.current?.contains(target) && !popoverRef.current?.contains(target)) close(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !event.isComposing) close();
    };
    // KT-2013 — a page scroll moves the trigger; the popover's own list
    // scrolling moves nothing: it is not placed again for it.
    const placeOnPageScroll = (event: Event) => {
      if (event.target instanceof Node && popoverRef.current?.contains(event.target)) return;
      place();
    };
    document.addEventListener('mousedown', closeOutside);
    document.addEventListener('keydown', closeOnEscape);
    window.addEventListener('resize', place);
    window.addEventListener('scroll', placeOnPageScroll, true);
    return () => {
      document.removeEventListener('mousedown', closeOutside);
      document.removeEventListener('keydown', closeOnEscape);
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', placeOnPageScroll, true);
    };
  }, [open, place, close]);

  return { open, setOpen, place, close, rootRef, popoverRef, searchRef, restoreFocusRef };
}

// ─── The grouped list: modes, then other models (KT-1994) ────────────────

type ListChoice =
  | { kind: 'tier'; target: AgentSwitchTarget; tier: ModelTier | null }
  | { kind: 'model'; target: AgentSwitchTarget; model: string; row: string };

/**
 * What is chosen now. With a `tier`, a model counts as that mode's only when it
 * is the model of that very mode (the discussion's rule); without one (the
 * exact-model mode) the first mode naming it wins, and no model is the agent's
 * own default.
 */
interface ListSelection {
  target: Pick<AgentSwitchTarget, 'agent' | 'connectionId'>;
  tier?: ModelTier | null;
  model?: string | null;
}

interface ListError { key: string; row: string; message: string }

const TRANSPORT_KIND: Record<AgentTransport, string> = { cli: 'cli', local: 'local', http: 'api' };

const catalogModels = (catalog: ModelCatalogSnapshot | null, target: AgentSwitchTarget): CatalogModelEntry[] => {
  const view = catalog?.targets.find(candidate => candidate.runtime_target_id === modelRuntimeTargetId(target.agent, target.connectionId));
  return view?.models.filter(model => model.runtime_target_id === view.runtime_target_id) ?? [];
};

/** Lowercased, letters and digits only: « GPT-6-Luna » and « gpt-6-luna » say the same. */
const comparableName = (value: string) => value.toLowerCase().replace(/[^\p{L}\p{N}]/gu, '');

/**
 * KT-1994 — the display name written next to an id, only when it says
 * something the id does not (« Fable », « Google: Gemini 4 Neon »).
 */
const modelName = (entry: CatalogModelEntry | undefined, id: string) => {
  const name = entry ? entry.display_alias ?? entry.display_name : '';
  return name && comparableName(name) !== comparableName(id) ? name : '';
};

/**
 * KT-2010 — the height of the band of the agent a row belongs to (0 outside
 * an agent, and for the band itself): what the band covers at the top of the
 * list while that agent's rows scroll under it.
 */
function stickyHeaderHeight(row: HTMLElement): number {
  const header = row.closest('.kr-agent-switch-group-block')
    ?.querySelector<HTMLElement>(':scope > .kr-agent-switch-group-header');
  return header && !header.contains(row) ? header.getBoundingClientRect().height : 0;
}

interface AgentModelListProps {
  ariaLabel: string;
  targets: readonly AgentSwitchTarget[];
  query: string;
  catalog: ModelCatalogSnapshot | null;
  tierOptions: readonly (ModelTier | null)[];
  resolve: (target: AgentSwitchTarget, tier: ModelTier) => ResolvedCatalogTier;
  tierLabel: (tier: ModelTier) => string;
  /** The `null` tier's full name (the agent's own default). */
  defaultLabel: string;
  /** A mode's tooltip name, `Tier · model`. */
  modeTitle: (target: AgentSwitchTarget, tier: ModelTier) => string;
  selection: ListSelection | null;
  /**
   * The agent a typed id is offered for, « as is » (the picker's current
   * agent): one entry at the end of the list, never one per agent.
   */
  freeTarget?: AgentSwitchTarget | null;
  /** Places the popover again before an unfolded agent is scrolled into view. */
  place?: () => void;
  /** The current choice cannot be chosen again (the discussion's picker). */
  disableSelected: boolean;
  /**
   * The exact-model mode greys the principal's own model; a mode row passes
   * its `tier` (the step's mode when the step names no model).
   */
  isMain?: (target: AgentSwitchTarget, model: string | null, tier?: ModelTier | null) => boolean;
  availability: (target: AgentSwitchTarget) => AgentAvailability | null;
  /** Other models, a custom model and a typed id. */
  models: boolean;
  /** `null`: no « Récents » section. */
  recents: readonly AgentPickerRecent[] | null;
  busy: boolean;
  error: ListError | null;
  onChoose: (choice: ListChoice) => void;
  /** Rows before the agents (« = principal »). */
  leading?: ReactNode;
  testId?: string;
  t: Translate;
  tp?: TranslatePlural;
}

function AgentModelList({
  ariaLabel, targets, query, catalog, tierOptions, resolve, tierLabel, defaultLabel, modeTitle,
  selection, freeTarget, place, disableSelected, isMain, availability, models, recents, busy, error, onChoose, leading,
  testId, t, tp: givenPlural,
}: AgentModelListProps) {
  const id = useId();
  // A translator without plural forms (a page's own) still names the count.
  const tp: TranslatePlural = givenPlural
    ?? ((key, count, ...args) => t(`${key}.${count === 1 ? 'one' : 'other'}`, count, ...args));
  // Folded for every agent; unfolding one leaves the others folded.
  const [unfolded, setUnfolded] = useState<ReadonlySet<string>>(() => new Set());
  const resultsRef = useRef<HTMLSpanElement>(null);
  // The agent just unfolded: its control goes to the top of the list's
  // viewport, so the rows it reveals are seen (the focus stays on it).
  const revealRef = useRef<string | null>(null);
  useLayoutEffect(() => {
    const key = revealRef.current;
    if (!key) return;
    revealRef.current = null;
    place?.();
    const list = resultsRef.current;
    const control = Array.from(list?.querySelectorAll<HTMLElement>('.kr-agent-switch-more-toggle') ?? [])
      .find(node => node.dataset.fold === key);
    if (!list || !control) return;
    // KT-2010 — right under its agent's band, which stays at the top.
    list.scrollTop += control.getBoundingClientRect().top - list.getBoundingClientRect().top - stickyHeaderHeight(control);
  });
  // KT-2010 — a row that takes the focus is never left under its agent's
  // band: the band sticks to the top of the list, so the row is scrolled to
  // just below it (the arrows, Home and End move the focus up the list).
  const keepFocusClearOfHeader = (event: ReactFocusEvent<HTMLElement>) => {
    const list = resultsRef.current;
    const row = event.target;
    if (!list || !(row instanceof HTMLElement)) return;
    const covered = list.getBoundingClientRect().top + stickyHeaderHeight(row) - row.getBoundingClientRect().top;
    if (covered > 0) list.scrollTop -= covered;
  };
  const typed = query.trim();
  const searching = typed.length > 0;
  const hits = (terms: readonly (string | null | undefined)[]) => matchesCatalogSearch(typed, [...terms]);

  const groups = targets.map(target => {
    const key = targetKey(target);
    const label = defaultTargetLabel(target);
    const listed = catalogModels(catalog, target);
    const modes = tierOptions.map(tier => {
      const resolution = tier === null ? null : resolve(target, tier);
      const modelId = modeModelId(resolution, tier);
      const entry = listed.find(model => model.model_id === modelId);
      return { tier, resolution, modelId, entry, name: modelName(entry, modelId) };
    });
    const modeIds = new Set(modes.map(mode => mode.modelId).filter(Boolean));
    const others = listed.filter(model => !modeIds.has(model.model_id));

    // Which row is the current choice.
    let selectedTier: ModelTier | null | undefined;
    let custom: string | null = null;
    if (selection && targetKey(selection.target) === key) {
      const held = selection.model?.trim();
      if (held) {
        const match = selection.tier !== undefined
          ? modes.find(mode => mode.tier === selection.tier && mode.modelId === held)
          : modes.find(mode => mode.tier !== null && mode.modelId === held);
        if (match) selectedTier = match.tier;
        else custom = held;
      } else {
        selectedTier = selection.tier === undefined ? null : selection.tier;
      }
    }

    // KT-1994 — with a search text, a mode row is shown when the text hits the
    // agent's name, that row's model (id or display name) or the mode's name;
    // other models by id or display name; an agent with no row is not listed.
    const agentTerms = [label, ...catalogTargetSearchTerms(target, [])];
    const shownModes = searching
      ? modes.filter(mode => hits([
        ...agentTerms, mode.modelId, mode.entry?.display_name, mode.entry?.display_alias, ...freeSearchTerms(mode.modelId, mode.entry?.display_name, t),
        ...(mode.tier === null ? [defaultLabel, t('agentPicker.defaultTier')] : [mode.tier, tierLabel(mode.tier)]),
      ]))
      : modes;
    const customEntry = custom ? listed.find(model => model.model_id === custom) : undefined;
    const customShown = Boolean(custom) && (!searching || hits([
      ...agentTerms, custom, customEntry?.display_name, customEntry?.display_alias, t('agentPicker.custom'),
      ...freeSearchTerms(custom, customEntry?.display_name, t),
    ]));
    const matchedOthers = models && searching
      ? others.filter(model => hits([model.model_id, model.display_name, model.display_alias, ...freeSearchTerms(model.model_id, model.display_name, t)]))
      : [];
    const known = listed.some(model => model.model_id === typed) || modeIds.has(typed) || custom === typed;
    return {
      target, key, label, listed, allModes: modes, modes: shownModes, others, matchedOthers, selectedTier, custom, customEntry, customShown, known,
      visible: !searching || shownModes.length > 0 || customShown || matchedOthers.length > 0,
    };
  });
  type Group = typeof groups[number];
  const visible = groups.filter(group => group.visible);
  // One typed id « as is », for the picker's current agent only, unless that agent already lists it.
  const freeGroup = models && searching && freeTarget
    ? groups.find(group => group.key === targetKey(freeTarget) && !group.known)
    : undefined;

  const errorAfter = (key: string, row: string) => error && error.key === key && error.row === row
    ? <span role="alert" className="kr-agent-switch-error">{error.message}</span>
    : null;

  /**
   * Why an agent cannot be chosen, written under its header (its badge alone
   * says « Indisponible »): the caller's detail, else the launch verdict's,
   * else what the catalogue says of its models.
   */
  const blockedReason = (group: Group, badge: AgentAvailability | null) => {
    if (!availabilityBlocks(badge)) return '';
    if (badge?.title) return badge.title;
    for (const mode of group.allModes) {
      const entry = mode.resolution?.entry;
      const reason = mode.resolution?.refusal || entry?.unavailable_detail
        || (entry?.unavailable_reason === 'not_found' || entry?.unavailable_reason === 'access_denied'
          ? t(`modelCatalog.reason.${entry.unavailable_reason}`) : '');
      if (reason) return reason;
    }
    return catalog?.targets.find(view =>
      view.runtime_target_id === modelRuntimeTargetId(group.target.agent, group.target.connectionId))?.last_error_detail ?? '';
  };

  const header = (group: Group, badge: AgentAvailability | null) => (<>
    <span className="kr-agent-switch-group kr-agent-switch-group-header">
      <span className="kr-agent-switch-tier-id">
        <span
          className="kr-agent-switch-option-dot"
          style={{ background: AGENT_COLORS[group.target.agent] ?? 'var(--kr-text-faint)' }}
        />
        <span className="kr-agent-switch-tier-name kr-agent-switch-group-name">{group.label}</span>
      </span>
      <span className="kr-agent-switch-kind" data-transport={agentTransport(group.target.agent, group.target.connectionId)}>
        {t(`agentPicker.kind.${TRANSPORT_KIND[agentTransport(group.target.agent, group.target.connectionId)]}`)}
      </span>
      <AgentAvailabilityBadge availability={badge} />
      {group.listed.length > 0 && (
        <span className="kr-agent-switch-model-count" data-count={group.listed.length}>
          {tp('agentPicker.modelCount', group.listed.length)}
        </span>
      )}
    </span>
    {blockedReason(group, badge) && (
      <span className="kr-agent-switch-reason" data-testid="agent-switch-reason">{blockedReason(group, badge)}</span>
    )}
  </>);

  /** « Identifiant absent du catalogue ? Utiliser « q » avec <agent> », at the end of the list. */
  const freeRow = (group: Group) => {
    const blocked = availabilityBlocks(availability(group.target));
    const text = t('agentPicker.freeModel', typed, group.label);
    return (
      <span className="kr-agent-switch-free-block">
        <button
          type="button"
          role="menuitem"
          className="kr-agent-switch-mode"
          data-kind="free"
          data-free="true"
          disabled={busy || blocked || Boolean(isMain?.(group.target, typed))}
          aria-label={text}
          onClick={() => onChoose({ kind: 'model', target: group.target, model: typed, row: 'free' })}
        >
          <span className="kr-agent-switch-mode-value">{text}</span>
        </button>
        {errorAfter(group.key, 'free')}
      </span>
    );
  };

  const renderGroup = (group: Group) => {
    const own = availability(group.target);
    const blocked = availabilityBlocks(own);
    const realTiers = group.allModes.flatMap(mode => mode.tier === null ? [] : [mode.tier]);
    const shared = sharedTierState(realTiers.map(tier => tierChoiceState(resolve(group.target, tier), t)));
    const isOpen = unfolded.has(group.key);
    const shownOthers = (searching ? group.matchedOthers : isOpen ? group.others : [])
      .filter(model => model.model_id !== group.custom);
    const mounted = shownOthers.slice(0, OTHER_MODELS_RENDER_LIMIT);
    const rest = shownOthers.length - mounted.length;
    return (
      <span key={group.key} className="kr-agent-switch-group-block" role="group" aria-label={group.label}>
        {header(group, rowAvailability(own, shared))}
        {group.modes.map(({ tier, resolution, modelId, name, entry }) => {
          const selected = group.selectedTier !== undefined && group.selectedTier === tier;
          const label = tier === null ? defaultLabel : tierLabel(tier);
          const title = tier === null ? defaultLabel : modeTitle(group.target, tier);
          const details = tierDetails(tier, resolution, title, t);
          const ownState = details.state && !sameAvailability(details.state, shared) ? details.state : null;
          const onMain = Boolean(isMain?.(group.target, modelId || null, tier));
          const descriptionId = `${id}-${group.key}-${tier ?? 'agent-default'}`;
          return (
            <button
              key={tier ?? 'agent-default'}
              type="button"
              role="menuitem"
              className="kr-agent-switch-mode"
              data-kind="mode"
              data-tier={tier ?? 'agent-default'}
              data-current={selected}
              data-main={onMain || undefined}
              aria-label={withFreeName(`${group.label} · ${label}`, modelId, entry?.display_name, t)}
              aria-describedby={descriptionId}
              title={onMain ? t('wf.shadow.rows.mainModel') : details.title}
              disabled={busy || blocked || details.blocked || onMain || (disableSelected && selected)}
              onClick={() => onChoose({ kind: 'tier', target: group.target, tier })}
            >
              <span className="kr-agent-switch-mode-label">
                {tier !== null && <span aria-hidden="true">{MODEL_TIER_ICONS[tier]}</span>}
                {/* « Modèle par défaut de l'agent » stays the name; the row shows « Défaut ». */}
                <span>{tier === null ? t('agentPicker.defaultTier') : label}</span>
              </span>
              <span className="kr-agent-switch-mode-value">
                {modelId
                  ? <code className="kr-agent-switch-mode-model">{modelId}</code>
                  : <span className="kr-agent-switch-mode-default">{t('wf.shadow.rows.defaultModel')}</span>}
                <FreeModelBadge modelId={modelId} displayName={entry?.display_name} t={t} />
                {name && <span className="kr-agent-switch-mode-name">{name}</span>}
                {ownState && <AgentAvailabilityBadge availability={ownState} placement="tier" />}
              </span>
              <span id={descriptionId} hidden>{details.description}</span>
              <span className="kr-agent-switch-mode-check">{selected && <Check size={10} aria-hidden="true" />}</span>
            </button>
          );
        })}
        {group.custom && group.customShown && (
          <Fragment key="custom">
            <button
              type="button"
              role="menuitem"
              className="kr-agent-switch-mode"
              data-kind="custom"
              data-current="true"
              aria-label={withFreeName(
                `${group.label} · ${t('agentPicker.custom')} · ${group.custom}`, group.custom, group.customEntry?.display_name, t,
              )}
              disabled={busy || blocked || disableSelected}
              onClick={() => group.custom && onChoose({ kind: 'model', target: group.target, model: group.custom, row: 'custom' })}
            >
              <span className="kr-agent-switch-mode-label"><span>{t('agentPicker.custom')}</span></span>
              <span className="kr-agent-switch-mode-value">
                <code className="kr-agent-switch-mode-model">{group.custom}</code>
                <FreeModelBadge modelId={group.custom} displayName={group.customEntry?.display_name} t={t} />
                {modelName(group.customEntry, group.custom) && (
                  <span className="kr-agent-switch-mode-name">
                    {modelName(group.customEntry, group.custom)}
                  </span>
                )}
              </span>
              <span className="kr-agent-switch-mode-check"><Check size={10} aria-hidden="true" /></span>
            </button>
            {errorAfter(group.key, 'custom')}
          </Fragment>
        )}
        {models && !searching && group.others.length > 0 && (
          <button
            type="button"
            role="menuitem"
            className="kr-agent-switch-more-toggle"
            data-fold={group.key}
            aria-expanded={isOpen}
            aria-label={`${group.label} · ${t('agentPicker.otherModels')}`}
            disabled={blocked}
            onClick={() => {
              if (!isOpen) revealRef.current = group.key;
              setUnfolded(previous => {
                const next = new Set(previous);
                if (next.has(group.key)) next.delete(group.key); else next.add(group.key);
                return next;
              });
            }}
          >
            {isOpen ? <ChevronDown size={10} aria-hidden="true" /> : <ChevronRight size={10} aria-hidden="true" />}
            <span>{t('agentPicker.otherModels')}</span>
            <span className="kr-agent-switch-model-count" data-count={group.others.length}>{group.others.length}</span>
          </button>
        )}
        {mounted.map(model => {
          const unavailable = model.availability === 'unavailable';
          const name = modelName(model, model.model_id);
          const onMain = Boolean(isMain?.(group.target, model.model_id));
          return (
            <Fragment key={model.model_id}>
              <button
                type="button"
                role="menuitem"
                className="kr-agent-switch-mode"
                data-kind="model"
                data-current="false"
                data-main={onMain || undefined}
                aria-label={withFreeName(`${group.label} · ${model.model_id}`, model.model_id, model.display_name, t)}
                title={onMain
                  ? t('wf.shadow.rows.mainModel')
                  : [name, unavailable ? t('modelCatalog.unavailable') : ''].filter(Boolean).join(' — ') || undefined}
                disabled={busy || blocked || unavailable || onMain}
                onClick={() => onChoose({ kind: 'model', target: group.target, model: model.model_id, row: `model:${model.model_id}` })}
              >
                <span className="kr-agent-switch-mode-label" />
                <span className="kr-agent-switch-mode-value">
                  <code className="kr-agent-switch-mode-model">{model.model_id}</code>
                  <FreeModelBadge modelId={model.model_id} displayName={model.display_name} t={t} />
                  {name && <span className="kr-agent-switch-mode-name">{name}</span>}
                  {unavailable && (
                    <AgentAvailabilityBadge
                      availability={{ state: 'unavailable', label: t('modelCatalog.unavailable') }}
                      placement="tier"
                    />
                  )}
                </span>
                <span className="kr-agent-switch-mode-check" />
              </button>
              {errorAfter(group.key, `model:${model.model_id}`)}
            </Fragment>
          );
        })}
        {rest > 0 && (
          <span className="kr-agent-switch-more" data-count={rest}>{t('agentPicker.moreModels', rest)}</span>
        )}
      </span>
    );
  };

  const recentRows = models && recents && !searching
    ? recents.flatMap(entry => {
      const target = targets.find(candidate => targetKey(candidate) === targetKey(entry));
      if (!target) return [];
      if (!entry.model && !tierOptions.includes(entry.tier ?? null)) return [];
      return [{ entry, target }];
    })
    : [];

  const renderRecent = ({ entry, target }: typeof recentRows[number]) => {
    const label = defaultTargetLabel(target);
    const blocked = availabilityBlocks(availability(target));
    const tier = entry.model ? undefined : entry.tier ?? null;
    const group = groups.find(candidate => candidate.key === targetKey(target));
    // KT-2013 — the row of the same choice in its group: its mode, else the
    // catalogue entry of its model. Its guards and its title are the recent's.
    const mode = tier === undefined ? undefined : group?.allModes.find(candidate => candidate.tier === tier);
    const modelId = entry.model ?? mode?.modelId ?? (tier ? modeModelId(resolve(target, tier), tier) : '');
    const choice = entry.model ? t('agentPicker.custom') : tier ? tierLabel(tier) : defaultLabel;
    const shown = modelId || t('wf.shadow.rows.defaultModel');
    const selected = Boolean(group) && (entry.model
      ? group?.custom === entry.model
      : group?.selectedTier !== undefined && group?.selectedTier === tier);
    // The recent row has no entry of its own: its group's catalogue entry for that id, when listed.
    const listedEntry = modelId ? group?.listed.find(model => model.model_id === modelId) : undefined;
    const displayName = listedEntry?.display_name;
    const onMain = Boolean(isMain?.(target, modelId || null, tier));
    let rowBlocked: boolean;
    let rowTitle: string | undefined;
    if (tier !== undefined) {
      const details = tierDetails(tier, mode?.resolution ?? null, tier === null ? defaultLabel : modeTitle(target, tier), t);
      rowBlocked = details.blocked;
      rowTitle = details.title;
    } else {
      const unavailable = listedEntry?.availability === 'unavailable';
      rowBlocked = unavailable;
      rowTitle = [modelName(listedEntry, modelId), unavailable ? t('modelCatalog.unavailable') : '']
        .filter(Boolean).join(' — ') || undefined;
    }
    const row = `recent:${recentKey(entry)}`;
    return (
      <Fragment key={recentKey(entry)}>
        <button
          type="button"
          role="menuitem"
          className="kr-agent-switch-mode kr-agent-switch-recent"
          data-kind="recent"
          data-current={selected}
          data-main={onMain || undefined}
          aria-label={withFreeName(`${t('agentPicker.recent')}: ${label} · ${choice} · ${shown}`, modelId, displayName, t)}
          title={onMain ? t('wf.shadow.rows.mainModel') : rowTitle}
          disabled={busy || blocked || rowBlocked || onMain || (disableSelected && selected)}
          onClick={() => onChoose(entry.model
            ? { kind: 'model', target, model: entry.model, row }
            : { kind: 'tier', target, tier: tier ?? null })}
        >
          <span className="kr-agent-switch-mode-label">
            <span
              className="kr-agent-switch-option-dot"
              style={{ background: AGENT_COLORS[target.agent] ?? 'var(--kr-text-faint)' }}
            />
            <span className="kr-agent-switch-tier-name">{label}</span>
          </span>
          <span className="kr-agent-switch-mode-value">
            <span className="kr-agent-switch-recent-choice">
              {tier && <span aria-hidden="true">{MODEL_TIER_ICONS[tier]}</span>}
              {choice}
            </span>
            {modelId
              ? <code className="kr-agent-switch-mode-model">{modelId}</code>
              : <span className="kr-agent-switch-mode-default">{shown}</span>}
            <FreeModelBadge modelId={modelId} displayName={displayName} t={t} />
          </span>
          <span className="kr-agent-switch-mode-check">{selected && <Check size={10} aria-hidden="true" />}</span>
        </button>
        {errorAfter(targetKey(target), row)}
      </Fragment>
    );
  };

  return (
    <>
      {!visible.length && <span role="status" className="kr-agent-switch-status">{t('agentPicker.noMatch')}</span>}
      <span
        ref={resultsRef}
        role="menu"
        aria-label={ariaLabel}
        className="kr-agent-switch-results"
        data-testid={testId}
        onFocus={keepFocusClearOfHeader}
      >
        {recentRows.length > 0 && (
          <span className="kr-agent-switch-recents" role="group" aria-label={t('agentPicker.recent')}>
            <span className="kr-agent-switch-group" role="presentation">{t('agentPicker.recent')}</span>
            {recentRows.map(renderRecent)}
          </span>
        )}
        {leading}
        {visible.map(renderGroup)}
        {freeGroup && freeRow(freeGroup)}
      </span>
    </>
  );
}

/**
 * Shared agent picker: discussion headers, workflow steps, the API assistants,
 * the shadows' exact model, and (through `AgentPickerPalette` and
 * `AgentPickerTierChoices`) the composer's mention menu. It owns only the
 * popover interaction; callers remain responsible for persisting the selected
 * agent and surfacing any API error.
 */
export function AgentSwitchPicker(props: AgentSwitchPickerProps) {
  if (props.exactModel) return <ExactModelPicker {...props} exactModel={props.exactModel} />;
  return <TierPicker {...props} />;
}

function TierPicker({
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
  onDefaultTierSelection,
  tierLabels,
  modelTiers,
  defaultModelLabel = 'Default agent model',
  disabled = false,
  compact = false,
  variant,
  title,
  ariaLabel,
  staticClassName,
  suffix,
  role,
  currentModelFallback,
  displayName,
  needsFullAccess,
  availability,
  multiple,
  customModel,
}: AgentSwitchPickerProps) {
  const { t, tp } = useT();
  const [query, setQuery] = useState('');
  const [saving, setSaving] = useState(false);
  const [modelError, setModelError] = useState<ListError | null>(null);
  const [recents, setRecents] = useState<AgentPickerRecent[]>([]);
  const savingRef = useRef(false);
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
  const choices = (multiple ? suppliedTargets : [currentTarget, ...suppliedTargets]).filter((target, index, targets) =>
    targets.findIndex(candidate => targetKey(candidate) === targetKey(target)) === index
  );
  const tierOptions: (ModelTier | null)[] = onDefaultTierSelection
    ? [null, ...TIER_CHOICES]
    : [...TIER_CHOICES];
  const tierPicker = !multiple && currentTier !== undefined
    && (onSelectionChange !== undefined || onTargetSelectionChange !== undefined);
  const canChange = multiple
    ? choices.length > 0
    : tierPicker
      ? choices.length > 1 || tierOptions.length > 1
      : Boolean(onChange) && choices.length > 1;
  const triggerVariant = variant ?? (compact ? 'compact' : 'chip');

  const { open, setOpen, place, close, rootRef, popoverRef, searchRef } = usePopover(query, saving);
  const { catalog, catalogError } = useModelCatalogSnapshot(open);
  // The catalogue's models and states change the list's size: placed again.
  useLayoutEffect(() => {
    if (open) place();
  }, [open, place, catalog, catalogError]);
  const navigateChoices = usePopoverKeys(popoverRef, searchRef, close);

  const targetLabel = defaultTargetLabel;
  const targetAvailability = (target: AgentSwitchTarget): AgentAvailability | null =>
    availability?.(target)
    ?? (needsFullAccess?.(target.agent)
      ? { state: 'needs-full-access', label: t('agentPicker.needsFullAccess') }
      : null);
  const resolvedTier = (target: AgentSwitchTarget, tier: ModelTier) => resolveCatalogTier(
    catalog, target, tier, modelTiers,
    targetKey(target) === targetKey(currentTarget) && tier === currentTier ? currentModel : null,
  );
  const targetModel = (target: AgentSwitchTarget, tier: ModelTier) =>
    resolvedTier(target, tier).model
    || (targetKey(target) === targetKey(currentTarget) && tier === currentTier
      ? currentModelFallback?.trim()
      : undefined)
    || defaultModelLabel;
  const tierLabel = (tier: ModelTier) => tierLabels?.[tier] ?? tier;
  const tierTitle = (target: AgentSwitchTarget, tier: ModelTier) =>
    `${tierLabel(tier)} · ${targetModel(target, tier)}`;
  // KT-1994 — a held exact model is « Personnalisé » unless it is the current mode's own model.
  const heldModel = tierPicker && customModel ? customModel.model?.trim() || '' : '';
  const custom = heldModel && heldModel !== (currentTier ? modeModelId(resolvedTier(currentTarget, currentTier), currentTier) : '')
    ? heldModel : '';
  const showsCurrentModel = Boolean(currentTier) && (tierPicker || role !== undefined);
  // KT-1994 — the trigger writes the model id the configuration names, whether
  // or not the catalogue is loaded (it is read at opening): its text never
  // changes when the list opens. The display name goes to the title.
  const configuredId = currentTier
    ? modeModelId(resolveCatalogTier(null, currentTarget, currentTier, modelTiers, currentModel), currentTier)
    : '';
  // KT-1936 — the model written next to the agent: the custom model, the
  // caller's suffix, the configured id, the model the agent last replied with,
  // else the explicit « default model of <agent> » state; never a blank, the
  // role or the tier alone.
  const identityModel: AgentIdentityModel | null = custom
    ? { kind: 'id', id: custom }
    : suffix?.trim()
      ? { kind: 'id', id: suffix.trim() }
      : showsCurrentModel && currentTier
        ? resolveIdentityModel({ mode: configuredId, lastReply: currentModelFallback })
        : null;
  const identityTier = tierPicker && currentTier && !custom ? currentTier : null;
  const identity = (name: string, color?: string) => identityModel ? (
    <AgentIdentity
      className="kr-agent-switch-identity"
      name={name}
      color={color}
      model={identityModel}
      roleLabel={role}
      tier={identityTier}
      tierLabel={identityTier ? tierTitle(currentTarget, identityTier) : undefined}
      truncateModel
    />
  ) : (
    <span className="kr-agent-switch-name" style={color ? { color } : undefined}>{name}</span>
  );
  const visibleChoices = choices.filter(target => matchesCatalogSearch(query, [
    targetLabel(target),
    ...catalogTargetSearchTerms(target, TIER_CHOICES.map(tier => resolvedTier(target, tier))),
    ...(tierPicker ? tierOptions.flatMap(tier => tier === null
      ? [defaultModelLabel]
      : [tier, tierLabels?.[tier]]) : []),
  ]));
  const isSelected = (target: AgentSwitchTarget) =>
    multiple?.selected.some(selected => targetKey(selected) === targetKey(target)) ?? false;

  const remember = (entry: AgentPickerRecent) => {
    if (customModel) setRecents(recordAgentPickerRecent(entry));
  };

  const select = async (target: AgentSwitchTarget, tier?: ModelTier | null) => {
    if (savingRef.current) return;
    if (tierPicker) {
      // The current mode again is a choice only while a custom model holds: it clears it.
      if (tier === undefined || (
        targetKey(target) === targetKey(currentTarget)
        && tier === currentTier
        && !custom
      )) return;
    } else if (!onChange || target.agent === currentAgent) {
      return;
    }
    savingRef.current = true;
    setSaving(true);
    setModelError(null);
    try {
      if (tierPicker && tier === null && onDefaultTierSelection) {
        await onDefaultTierSelection(target);
      } else if (tierPicker && tier && onTargetSelectionChange) {
        await onTargetSelectionChange(target, tier);
      } else if (tierPicker && tier && onSelectionChange) {
        await onSelectionChange(target.agent, tier);
      } else if (onChange) {
        await onChange(target.agent);
      }
      if (tierPicker && tier !== undefined) remember({ agent: target.agent, connectionId: target.connectionId, tier });
      close();
    } catch {
      // The caller owns the user-facing error. Keep the picker open so
      // another available combination can be selected.
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  /** KT-1994 — an exact model: the server's refusal is shown next to the entry, the choice stays. */
  const selectModel = async (target: AgentSwitchTarget, model: string, row: string) => {
    const chosen = model.trim();
    if (savingRef.current || !customModel || !chosen) return;
    if (targetKey(target) === targetKey(currentTarget) && chosen === custom) return;
    savingRef.current = true;
    setSaving(true);
    setModelError(null);
    try {
      await customModel.onSelect(target, chosen);
      remember({ agent: target.agent, connectionId: target.connectionId, model: chosen });
      close();
    } catch (error) {
      setModelError({ key: targetKey(target), row, message: userError(error) });
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  };

  // KT-1936 — the identity as words: agent, the FULL model id (or its unknown
  // state), the mode or « Personnalisé », the role. The visible model is cut
  // past 17 characters; this text is the trigger's accessible name after its
  // action, and the static form's tooltip.
  const identityText = (name: string) => identityModel ? agentIdentityText({
    name,
    model: identityModel,
    tierLabel: custom ? t('agentPicker.custom') : identityTier ? tierLabel(identityTier) : null,
    roleLabel: role ?? null,
  }, t) : null;

  if (!canChange) {
    const name = displayName ?? targetLabel(currentTarget);
    return (
      <span className={staticClassName ?? 'kr-agent-switch-static'} title={identityText(name) ?? undefined}>
        {identity(name, agentTextColor(currentTarget.agent))}
      </span>
    );
  }

  // Multi-choice: « No agent », or a count with its own singular and plural form.
  const triggerText = multiple
    ? (multiple.summary?.(multiple.selected)
      ?? (multiple.selected.length === 0
        ? t('agentPicker.selectedNone')
        : tp('agentPicker.selectedCount', multiple.selected.length)))
    : displayName ?? targetLabel(currentTarget);

  return (
    <span
      ref={rootRef}
      className="kr-agent-switch"
      data-compact={triggerVariant === 'compact'}
      data-variant={triggerVariant}
      data-open={open}
      onClick={event => event.stopPropagation()}
    >
      <button
        type="button"
        className="kr-agent-switch-btn"
        style={multiple ? undefined : { ['--agent-switch-tone' as string]: agentColor(currentTarget.agent) }}
        title={custom
          ? `${title} · ${t('agentPicker.custom')} · ${custom}`
          : tierPicker && currentTier
            ? `${title} · ${tierTitle(currentTarget, currentTier)}`
            : title}
        aria-label={!multiple && identityText(triggerText)
          ? `${ariaLabel} — ${identityText(triggerText)}`
          : ariaLabel}
        aria-haspopup="dialog"
        aria-expanded={open}
        disabled={disabled || saving}
        onClick={() => {
          if (!open) {
            setQuery('');
            setModelError(null);
            if (customModel) setRecents(readAgentPickerRecents());
          }
          setOpen(value => !value);
        }}
      >
        {saving ? <Loader2 size={9} className="spin" /> : <RefreshCw size={9} />}
        {multiple
          ? <span className="kr-agent-switch-name">{triggerText}</span>
          : identity(triggerText, agentTextColor(currentTarget.agent))}
        {custom && <span className="kr-agent-switch-custom">{t('agentPicker.custom')}</span>}
        <ChevronDown size={12} aria-hidden="true" />
      </button>
      {open && createPortal(
        <span
          ref={popoverRef}
          className="kr-agent-switch-popover"
          role="dialog"
          aria-label={ariaLabel}
          onKeyDown={navigateChoices}
        >
          <SearchField
            ref={searchRef}
            className="kr-agent-switch-search"
            size="sm"
            label={t('agentPicker.search')}
            placeholder={t('agentPicker.search')}
            value={query}
            onChange={setQuery}
          />
          {catalogError && <span role="status" className="kr-agent-switch-status">{t('modelCatalog.loadError')}</span>}
          {tierPicker ? (
            <AgentModelList
              ariaLabel={ariaLabel}
              targets={choices}
              query={query}
              catalog={catalog}
              tierOptions={tierOptions}
              resolve={resolvedTier}
              tierLabel={tierLabel}
              defaultLabel={defaultModelLabel}
              modeTitle={tierTitle}
              selection={{ target: currentTarget, tier: currentTier ?? null, model: custom || null }}
              freeTarget={currentTarget}
              place={place}
              disableSelected
              availability={targetAvailability}
              models={Boolean(customModel)}
              recents={customModel ? recents : null}
              busy={saving}
              error={modelError}
              onChoose={choice => void (choice.kind === 'tier'
                ? select(choice.target, choice.tier)
                : selectModel(choice.target, choice.model, choice.row))}
              t={t}
              tp={tp}
            />
          ) : (<>
          {!visibleChoices.length && <span role="status" className="kr-agent-switch-status">{t('agentPicker.noMatch')}</span>}
          <span role="menu" aria-label={ariaLabel} className="kr-agent-switch-results">
          {visibleChoices.map(target => {
            const badge = targetAvailability(target);
            const blocked = availabilityBlocks(badge);
            if (multiple) {
              const checked = isSelected(target);
              return (
                <button
                  key={targetKey(target)}
                  type="button"
                  role="menuitemcheckbox"
                  aria-checked={checked}
                  className="kr-agent-switch-option"
                  data-current={checked}
                  disabled={blocked}
                  onClick={() => multiple.onToggle(target)}
                >
                  <span className="kr-agent-switch-check" data-checked={checked} aria-hidden="true">
                    {checked && <Check size={9} />}
                  </span>
                  <span
                    className="kr-agent-switch-option-dot"
                    style={{ background: AGENT_COLORS[target.agent] ?? 'var(--kr-text-faint)' }}
                  />
                  {targetLabel(target)}
                  <AgentAvailabilityBadge availability={badge} />
                </button>
              );
            }
            const current = targetKey(target) === targetKey(currentTarget);
            return (
              <button
                key={targetKey(target)}
                type="button"
                role="menuitem"
                className="kr-agent-switch-option"
                data-current={current}
                disabled={saving || current || blocked}
                onClick={() => void select(target)}
              >
                <span
                  className="kr-agent-switch-option-dot"
                  style={{ background: AGENT_COLORS[target.agent] ?? 'var(--kr-text-faint)' }}
                />
                {targetLabel(target)}
                <AgentAvailabilityBadge availability={badge} />
                {current && <Check size={10} />}
              </button>
            );
          })}
          </span>
          </>)}
        </span>,
        document.body,
      )}
    </span>
  );
}

/** The agents offered when the page supplies none: every agent Kronn knows. */
const FALLBACK_TARGETS: readonly AgentSwitchTarget[] = [...ALL_AGENT_TYPES, 'Custom' as AgentType].map(agent => ({ agent }));

function ExactModelPicker({
  availableTargets, modelTiers, ariaLabel, disabled = false, compact = false, variant, availability,
  needsFullAccess, exactModel,
}: AgentSwitchPickerProps & { exactModel: ExactModelSelection }) {
  const { t, tp } = useT();
  const { value, onChange, main, testId, children, className } = exactModel;
  const [query, setQuery] = useState('');
  const [recents, setRecents] = useState<AgentPickerRecent[]>([]);
  const supplied = availableTargets?.length ? availableTargets : FALLBACK_TARGETS;
  // A saved connection the page no longer lists stays choosable, under its id.
  const listed: readonly AgentSwitchTarget[] = value?.connection_id
    && !supplied.some(target => targetKey(target) === targetKey({ agent: value.agent, connectionId: value.connection_id }))
    ? [...supplied, { agent: value.agent, connectionId: value.connection_id, label: value.connection_id }]
    : supplied;
  const valueKey = value ? targetKey({ agent: value.agent, connectionId: value.connection_id }) : null;
  // KT-1994 — the chosen agent first, then the configured order.
  const targets = valueKey
    ? [...listed.filter(target => targetKey(target) === valueKey), ...listed.filter(target => targetKey(target) !== valueKey)]
    : listed;
  const { open, setOpen, place, close, rootRef, popoverRef, searchRef } = usePopover(query);
  const { catalog, catalogError } = useModelCatalogSnapshot(open);
  useLayoutEffect(() => {
    if (open) place();
  }, [open, place, catalog, catalogError]);
  const navigate = usePopoverKeys(popoverRef, searchRef, close);
  const triggerVariant = variant ?? (compact ? 'compact' : 'chip');

  const resolve = (target: AgentSwitchTarget, tier: ModelTier) => resolveCatalogTier(catalog, target, tier, modelTiers);
  const tierLabel = (tier: ModelTier) => t(`disc.tier.${tier}`);
  const defaultLabel = t('wf.shadow.rows.defaultModel');
  const valueTarget = value
    ? targets.find(target => targetKey(target) === valueKey) ?? { agent: value.agent, connectionId: value.connection_id }
    : null;
  // « Personnalisé »: a model no mode of its agent points to.
  const custom = value?.model && valueTarget
    && !TIER_CHOICES.some(tier => modeModelId(resolve(valueTarget, tier), tier) === value.model)
    ? value.model : '';
  const valueTier = value?.model && valueTarget && !custom
    ? TIER_CHOICES.find(tier => modeModelId(resolve(valueTarget, tier), tier) === value.model)
    : undefined;

  const fullLabel = agentModelLabel(value, main, targets, t, modelTiers);
  const pick = (next: ShadowAgent | undefined, recent?: AgentPickerRecent) => {
    close();
    if (recent) setRecents(recordAgentPickerRecent(recent));
    onChange(next);
  };
  // KT-2013 — what the step runs: its explicit model, else the model of its
  // mode; a mode that names none is greyed as that mode, not as the agent's default.
  const mainKey = targetKey(main);
  const mainTier = main.tier ?? null;
  const mainTarget = targets.find(target => targetKey(target) === mainKey) ?? { agent: main.agent, connectionId: main.connectionId };
  const mainModel = main.model?.trim()
    || (mainTier ? modeModelId(resolve(mainTarget, mainTier), mainTier) : '');
  const isMain = (target: AgentSwitchTarget, model: string | null, tier?: ModelTier | null) =>
    targetKey(target) === mainKey && (mainModel
      ? (model ?? '') === mainModel
      : tier !== undefined && tier === mainTier);
  const choose = (choice: ListChoice) => {
    const connectionId = choice.target.connectionId ?? null;
    const model = choice.kind === 'model' ? choice.model.trim() : modeModelId(
      choice.tier === null ? null : resolve(choice.target, choice.tier), choice.tier);
    pick({
      agent: choice.target.agent,
      ...(model ? { model } : {}),
      ...(connectionId ? { connection_id: connectionId } : {}),
    }, choice.kind === 'model'
      ? { agent: choice.target.agent, connectionId, model }
      : { agent: choice.target.agent, connectionId, tier: choice.tier });
  };

  return (
    <span
      ref={rootRef}
      className={`kr-agent-switch wf-shadow-picker${className ? ` ${className}` : ''}`}
      data-compact={triggerVariant === 'compact'}
      data-variant={triggerVariant}
      data-mode="exact-model"
      data-open={open}
      onClick={event => event.stopPropagation()}
    >
      <button
        type="button"
        className="kr-agent-switch-btn wf-shadow-picker-btn"
        data-testid={testId}
        // KT-1936 — the action, then what the trigger shows (agent and model,
        // « Personnalisé »): an aria-label replaces the button's own text.
        aria-label={children
          ? ariaLabel
          : `${ariaLabel} — ${[fullLabel, custom ? t('agentPicker.custom') : ''].filter(Boolean).join(' · ')}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        title={children ? undefined : [fullLabel, custom ? t('agentPicker.custom') : ''].filter(Boolean).join(' · ')}
        disabled={disabled}
        onClick={() => {
          if (!open) {
            setQuery('');
            setRecents(readAgentPickerRecents());
          }
          setOpen(value => !value);
        }}
      >
        {children ?? <span className="kr-agent-switch-name">{fullLabel}</span>}
        {!children && custom && <span className="kr-agent-switch-custom">{t('agentPicker.custom')}</span>}
        {!children && valueTier && (
          <span className="kr-agent-switch-current-tier" data-tier={valueTier} title={tierLabel(valueTier)}>
            {MODEL_TIER_ICONS[valueTier]}
          </span>
        )}
        {!children && !value && <em className="wf-shadow-picker-hint">{t('wf.shadow.rows.sameAgent')}</em>}
        <ChevronDown size={9} aria-hidden />
      </button>
      {open && createPortal(
        <span
          ref={popoverRef}
          className="kr-agent-switch-popover"
          data-mode="exact-model"
          role="dialog"
          aria-label={ariaLabel}
          onKeyDown={navigate}
        >
          <SearchField
            ref={searchRef}
            className="kr-agent-switch-search"
            size="sm"
            label={t('agentPicker.searchModel')}
            placeholder={t('agentPicker.searchModel')}
            value={query}
            onChange={setQuery}
          />
          {/* KT-2013 — the list without its models says why, like the tier picker's. */}
          {catalogError && <span role="status" className="kr-agent-switch-status">{t('modelCatalog.loadError')}</span>}
          <AgentModelList
            ariaLabel={ariaLabel}
            testId={`${testId}-menu`}
            targets={targets}
            query={query}
            catalog={catalog}
            tierOptions={[null, ...TIER_CHOICES]}
            resolve={resolve}
            tierLabel={tierLabel}
            defaultLabel={defaultLabel}
            modeTitle={(target, tier) => `${tierLabel(tier)} · ${modeModelId(resolve(target, tier), tier) || defaultLabel}`}
            selection={value && valueTarget ? { target: valueTarget, model: value.model ?? null } : null}
            // The chosen agent, else the step's own (« = principal »).
            freeTarget={valueTarget
              ?? targets.find(target => targetKey(target) === targetKey(main))
              ?? { agent: main.agent, connectionId: main.connectionId }}
            place={place}
            disableSelected={false}
            isMain={isMain}
            availability={target => availability?.(target)
              ?? (needsFullAccess?.(target.agent) ? { state: 'needs-full-access', label: t('agentPicker.needsFullAccess') } : null)}
            models
            recents={recents}
            busy={false}
            error={null}
            onChoose={choose}
            leading={(
              <button
                type="button"
                role="menuitem"
                className="kr-agent-switch-option kr-agent-switch-model"
                data-current={!value}
                data-testid={`${testId}-same`}
                onClick={() => pick(undefined)}
              >
                <span>{t('wf.shadow.rows.sameAgent')}</span>
                <span className="kr-agent-switch-hint">{t('wf.shadow.rows.sameAgentHint')}</span>
                {!value && <Check size={10} aria-hidden />}
              </button>
            )}
            t={t}
            tp={tp}
          />
        </span>,
        document.body,
      )}
    </span>
  );
}

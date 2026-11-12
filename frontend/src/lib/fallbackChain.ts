import { AGENT_LABELS } from './constants';
import type {
  AgentFallback,
  AgentType,
  ChainView,
  FallbackMode,
  FallbackTransport,
  LinkView,
  ModelTier,
} from '../types/generated';

/** The longest chain the backend accepts (`MAX_CHAIN_DEPTH`). */
export const MAX_FALLBACK_CHAIN = 4;

/** The three text modes: the quality tiers (eco = economy, standard = default, advanced = reasoning). */
export const TEXT_MODES: readonly FallbackMode[] = ['eco', 'standard', 'advanced'];

export const ALL_MODES: readonly FallbackMode[] = ['eco', 'standard', 'advanced', 'decision', 'image', 'video'];

export type FallbackKind = 'text' | 'decision' | 'image' | 'video';

/** What a mode produces: a fallback only ever goes to the same kind. */
export const modeKind = (mode: FallbackMode): FallbackKind =>
  mode === 'eco' || mode === 'standard' || mode === 'advanced' ? 'text' : mode;

export const modeOfTier = (tier: ModelTier | null | undefined): FallbackMode =>
  tier === 'economy' ? 'eco' : tier === 'reasoning' ? 'advanced' : 'standard';

/** How an agent is reached: a remote HTTP API, a local model server, or a CLI process. */
export const transportOf = (agent: AgentType): FallbackTransport =>
  agent === 'Ollama' ? 'local' : agent === 'LiteLlm' || agent === 'Nvidia' || agent === 'Custom' ? 'http' : 'cli';

export const TRANSPORT_LABEL: Record<FallbackTransport, string> = { cli: 'CLI', http: 'HTTP', local: 'Local' };

const label = (agent: AgentType) => AGENT_LABELS[agent] ?? agent;

/** The mode a link runs in: its own, else the one it replaces. */
export const linkMode = (link: Pick<AgentFallback, 'mode'>, replaced: FallbackMode): FallbackMode =>
  link.mode ?? replaced;

/** One thing a fallback can be: an agent, a model of it and the mode that model serves. */
export interface FallbackOption {
  agent: AgentType;
  /** `null`: the agent's own model for the mode (the row's mode only). */
  model: string | null;
  mode: FallbackMode;
}

export const transportOfOption = (option: FallbackOption): FallbackTransport => transportOf(option.agent);

export const toLink = (option: FallbackOption, replaced: FallbackMode): AgentFallback => ({
  agent: option.agent,
  model: option.model,
  // Said only when it differs from the row it replaces: the same mode is the default.
  ...(option.mode !== replaced ? { mode: option.mode } : {}),
});

/** Same agent, same model, same mode. */
export const sameTarget = (
  a: Pick<AgentFallback, 'agent' | 'model'> & { mode?: FallbackMode | null },
  b: Pick<AgentFallback, 'agent' | 'model'> & { mode?: FallbackMode | null },
  replaced: FallbackMode,
): boolean =>
  a.agent === b.agent && (a.model ?? null) === (b.model ?? null) && linkMode({ mode: a.mode }, replaced) === linkMode({ mode: b.mode }, replaced);

/** What the menu of a row offers: the options of the same kind as `mode`, never
 *  the agent the row belongs to. An option with no explicit model of its own is
 *  offered on the row's own mode only: a model-less link runs the mode of the
 *  turn it replaces, so another mode would not be what the pill says. */
export function optionsFor(all: FallbackOption[], origin: AgentType | null, mode: FallbackMode): FallbackOption[] {
  return all.filter((option) =>
    option.agent !== origin
    && modeKind(option.mode) === modeKind(mode)
    && (option.model !== null || option.mode === mode));
}

/** What is wrong with `chain` whatever the configuration: the same rules the
 *  backend refuses on, so the owner sees them before anything is sent. */
export function chainProblem(
  origin: AgentType | null,
  mode: FallbackMode,
  chain: AgentFallback[],
): { key: string; args: string[] } | null {
  if (chain.length > MAX_FALLBACK_CHAIN) return { key: 'fallback.tooDeep', args: [String(MAX_FALLBACK_CHAIN)] };
  const seen: AgentType[] = origin ? [origin] : [];
  for (const link of chain) {
    if (link.agent === origin) return { key: 'fallback.self', args: [label(link.agent)] };
    if (seen.includes(link.agent)) return { key: 'fallback.loop', args: [label(link.agent)] };
    if (link.mode && modeKind(link.mode) !== modeKind(mode)) {
      return { key: 'fallback.wrongKind', args: [mode, link.mode] };
    }
    seen.push(link.agent);
  }
  return null;
}

/** The fallback set on the `mode` row of `agent`, as the overview served it. */
export const rowOf = (chains: ChainView[] | undefined, agent: AgentType, mode: FallbackMode): LinkView | undefined =>
  chains?.find((chain) => chain.agent === agent && chain.mode === mode)?.links[0];

export interface InheritedLink {
  /** The agent whose card holds this fallback. */
  source: AgentType;
  link: LinkView;
}

/** The fallbacks that follow `links` through the rows of the fallbacks — the
 *  same walk as the backend: a missing row, an agent already on the walk or the
 *  depth limit ends it. */
export function inheritedAfter(
  chains: ChainView[] | undefined,
  origin: AgentType | null,
  mode: FallbackMode,
  links: Pick<AgentFallback, 'agent' | 'mode'>[],
): InheritedLink[] {
  if (!chains || links.length === 0) return [];
  const seen: AgentType[] = origin ? [origin] : [];
  let at = mode;
  for (const link of links) {
    seen.push(link.agent);
    at = linkMode(link, at);
  }
  let tail = links[links.length - 1].agent;
  const out: InheritedLink[] = [];
  while (links.length + out.length < MAX_FALLBACK_CHAIN) {
    const next = rowOf(chains, tail, at);
    if (!next || seen.includes(next.agent)) break;
    out.push({ source: tail, link: next });
    seen.push(next.agent);
    tail = next.agent;
    at = next.mode;
  }
  return out;
}

const MENU_MARGIN = 8;
const MENU_GAP = 6;
const MENU_MAX_HEIGHT = 320;

export interface MenuPlacement {
  left: number;
  top: number;
  maxHeight: number;
}

/** Where the add / change menu goes so it is never clipped: under its anchor and
 *  left-aligned when there is room, flipped onto the anchor's right edge when it
 *  would cross the viewport's right side, above the anchor when there is more room
 *  above than below. */
export function placeMenu(
  anchor: { left: number; right: number; top: number; bottom: number },
  size: { width: number; height: number },
  viewport: { width: number; height: number },
): MenuPlacement {
  const width = Math.min(size.width, viewport.width - 2 * MENU_MARGIN);
  let left = anchor.left;
  if (left + width > viewport.width - MENU_MARGIN) left = anchor.right - width;
  left = Math.max(MENU_MARGIN, Math.min(left, viewport.width - MENU_MARGIN - width));
  const below = viewport.height - anchor.bottom - MENU_GAP - MENU_MARGIN;
  const above = anchor.top - MENU_GAP - MENU_MARGIN;
  const flipUp = size.height > below && above > below;
  const room = flipUp ? above : below;
  const maxHeight = Math.max(120, Math.min(MENU_MAX_HEIGHT, room));
  const height = Math.min(size.height, maxHeight);
  const top = flipUp ? Math.max(MENU_MARGIN, anchor.top - MENU_GAP - height) : anchor.bottom + MENU_GAP;
  return { left, top, maxHeight };
}

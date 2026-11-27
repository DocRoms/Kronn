// KT-1885 — why an agent choice cannot run (or runs with a warning). Every
// place that chooses an agent shows it through `AgentAvailabilityBadge`
// (components/AgentSwitchPicker.tsx), so a state reads the same in a header, a
// step, an assistant and the composer's @ menu.
import type { ResolvedCatalogTier } from './modelCatalogSelection';

type Translate = (key: string, ...args: (string | number)[]) => string;

/**
 * `unavailable` and `disabled` block the choice; `needs-full-access` and
 * `not-listed` warn; `catalog` is only what the catalogue says of the model
 * (« Détecté », « Cache à revérifier », « Configuration migrée »), neutral.
 */
export type AgentAvailabilityState = 'unavailable' | 'disabled' | 'needs-full-access' | 'not-listed' | 'catalog';

export interface AgentAvailability {
  state: AgentAvailabilityState;
  label: string;
  /** The detail, in the badge's title (a refusal, a notice). */
  title?: string;
}

export const availabilityBlocks = (availability?: AgentAvailability | null) =>
  availability?.state === 'unavailable' || availability?.state === 'disabled';

/** What the catalogue says about one tier of one agent, as an availability. */
export function tierAvailability(resolution: ResolvedCatalogTier | null | undefined, t: Translate): AgentAvailability | null {
  if (!resolution) return null;
  if (resolution.unavailable) {
    return { state: 'unavailable', label: t('modelCatalog.unavailable'), title: resolution.refusal ?? undefined };
  }
  if (resolution.notice) {
    return { state: 'not-listed', label: `⚠ ${t('modelCatalog.notListedBadge')}`, title: resolution.notice };
  }
  return null;
}

/**
 * KT-1885 — everything a tier choice can say about its model: its availability,
 * or else where the catalogue knows the model from (`catalog`).
 */
export function tierChoiceState(resolution: ResolvedCatalogTier | null | undefined, t: Translate): AgentAvailability | null {
  const availability = tierAvailability(resolution, t);
  if (availability || !resolution) return availability;
  if (resolution.entry && resolution.provenance) {
    return { state: 'catalog', label: t(`modelCatalog.provenance.${resolution.provenance}`) };
  }
  return resolution.configured ? { state: 'catalog', label: t('modelCatalog.notInCatalog') } : null;
}

const stateKey = (state: AgentAvailability | null | undefined) => state ? `${state.state}:${state.label}` : '';

/** Two states read the same (same kind, same words). */
export const sameAvailability = (a: AgentAvailability | null | undefined, b: AgentAvailability | null | undefined) =>
  stateKey(a) === stateKey(b);

/**
 * The state most of a row's tiers share (more than half of them), written once
 * next to the agent; a tier whose state differs carries its own. `null` when
 * no state is shared that way.
 */
export function sharedTierState(states: readonly (AgentAvailability | null)[]): AgentAvailability | null {
  const counts = new Map<string, { state: AgentAvailability; count: number }>();
  for (const state of states) {
    if (!state) continue;
    const key = stateKey(state);
    const entry = counts.get(key);
    if (entry) entry.count += 1;
    else counts.set(key, { state, count: 1 });
  }
  for (const { state, count } of counts.values()) {
    if (count * 2 > states.length) return state;
  }
  return null;
}

const SEVERITY: Record<AgentAvailabilityState, number> = {
  unavailable: 3, disabled: 3, 'needs-full-access': 2, 'not-listed': 2, catalog: 1,
};

/** A row shows one badge: the more serious of the agent's own state and its tiers' shared one. */
export function rowAvailability(
  target: AgentAvailability | null | undefined,
  shared: AgentAvailability | null | undefined,
): AgentAvailability | null {
  if (!target) return shared ?? null;
  if (!shared) return target;
  return SEVERITY[shared.state] > SEVERITY[target.state] ? shared : target;
}

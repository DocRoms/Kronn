// Batch loader for the sidebar weight indicator.
//
// Two guarantees the callers depend on:
//  * disabled in config  → NOT a single batch request is issued;
//  * never a fabricated zero → an unfinished or failed load reports its own
//    state, so the badge can stay wordless instead of claiming "0 o".
//
// State is DERIVED from what has been loaded, keyed by the request. Pushing
// 'loading' from inside the effect would cascade renders (and React's lint
// rejects it), so results carry the key they belong to and the state falls out
// of comparing that key with the current one.
import { useEffect, useMemo, useRef, useState } from 'react';
import type { DiscussionWeightView } from '../types/generated';
import { config as configApi } from './api';
import type { WeightLoadState } from './weightFormat';

/** Server cap, mirrored from `db::media_jobs`. Exported so callers bound
 * their selection rather than relying on the slicing below as a safety net. */
export const MAX_BATCH = 200;

/** Stable identity so an empty result never retriggers a consumer's effects. */
const EMPTY: Record<string, DiscussionWeightView> = {};

export interface DiscussionWeightsResult {
  /** False keeps the badge out of the DOM entirely. */
  enabled: boolean;
  /** State of the batch as a whole. Prefer `stateFor` per row. */
  state: WeightLoadState;
  weights: Record<string, DiscussionWeightView>;
  /** State for ONE discussion. An id outside the requested batch reports
   * `unmeasured`, never a settled zero. */
  stateFor: (id: string) => WeightLoadState;
}

type Outcome =
  | { key: string; ok: true; weights: Record<string, DiscussionWeightView> }
  | { key: string; ok: false };

/** Selection a list view asks for, bounded at the source so the endpoint's own
 * cap stays a backstop rather than the mechanism.
 *
 * The rows BEYOND this bound are not measured, and callers must render them as
 * `unmeasured` — the bound limits the query, not the list, so treating the
 * remainder as empty would invent a measurement. */
export function boundedWeightIds(items: { id: string }[]): string[] {
  return items.slice(0, MAX_BATCH).map(item => item.id);
}

function chunk<T>(items: T[], size: number): T[][] {
  const out: T[][] = [];
  for (let i = 0; i < items.length; i += size) out.push(items.slice(i, i + size));
  return out;
}

export function useDiscussionWeights(discussionIds: string[]): DiscussionWeightsResult {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);

  // Sorted + joined: a re-render that reorders the same ids must not refetch.
  const key = useMemo(() => [...discussionIds].sort().join(','), [discussionIds]);
  const latestRequest = useRef(0);
  // Exactly what was asked for, so a row outside the batch can say so instead
  // of inheriting the batch's settled state.
  const requested = useMemo(() => new Set(key ? key.split(',') : []), [key]);

  useEffect(() => {
    let cancelled = false;
    configApi
      .getServerConfig()
      .then(cfg => {
        if (!cancelled) setEnabled(cfg.discussion_weight?.enabled ?? false);
      })
      .catch(() => {
        // Cannot tell whether the indicator is on: stay silent rather than
        // guess, and issue no batch call.
        if (!cancelled) setEnabled(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    // The guard is the point of the feature: disabled means zero request.
    if (enabled !== true || key === '') return;
    const ids = key.split(',');
    const request = ++latestRequest.current;

    Promise.all(chunk(ids, MAX_BATCH).map(slice => configApi.discussionWeights(slice)))
      .then(pages => {
        if (request !== latestRequest.current) return;
        const merged: Record<string, DiscussionWeightView> = {};
        for (const page of pages) Object.assign(merged, page.weights);
        setOutcome({ key, ok: true, weights: merged });
      })
      .catch(() => {
        if (request !== latestRequest.current) return;
        setOutcome({ key, ok: false });
      });
  }, [enabled, key]);

  // `null` is "config not read yet", which is NOT the same as "disabled":
  // reporting `ready` here would let the badge render a measured-looking 0.
  // A result keyed on another selection is stale — that batch is still in
  // flight.
  const batchState: WeightLoadState =
    enabled === null
      ? 'loading'
      : enabled === false || key === ''
        ? 'ready'
        : outcome === null || outcome.key !== key
          ? 'loading'
          : outcome.ok
            ? 'ready'
            : 'unavailable';

  const weights =
    outcome !== null && outcome.key === key && outcome.ok ? outcome.weights : EMPTY;

  return {
    enabled: enabled === true,
    state: batchState,
    weights,
    stateFor: (id: string) => {
      if (batchState !== 'ready') return batchState;
      // Bounding the request does not make the rows beyond the bound weigh
      // nothing: they were never measured, and must not render a 0.
      return requested.has(id) ? 'ready' : 'unmeasured';
    },
  };
}

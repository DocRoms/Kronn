import { useCallback, useEffect, useRef, useState } from 'react';
import { runsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { RunStatusCard } from './RunStatusCard';
import { sharedRunStatusCardModel } from '../lib/runStatusCardModel';
import type { SharedRun } from '../types/generated';

/** A `shared_run_updated` event, identified by the run it targets, plus a
 * monotonic sequence so a repeated event for the same run_id (or an event
 * that arrives before this component has mounted) is never mistaken for a
 * new one. */
export type RunEventHint = { runId: string; seq: number };

const RELIST_DEBOUNCE_MS = 250;

/**
 * A run started elsewhere (QP batch, QA/QE launch, Workflow) that gets
 * attached to this discussion via its `discussion_id` shows up here
 * automatically, through the same server model and card used everywhere
 * else — no discussion-specific rendering logic.
 *
 * `runEvent` is forwarded by the page's single `useWebSocket` subscription
 * on `shared_run_updated` — this component deliberately does not open its
 * own socket subscription (no duplicated live logic, DoD #2/#6). A known
 * run_id is already isolated by its own `RunStatusCard` (it self-hydrates
 * via its own scoped subscription), so only an event for a run_id NOT yet
 * in the attached list triggers a relist — and a burst of such events is
 * debounced into a single relist instead of one per event.
 */
export function DiscussionAttachedRuns({ discussionId, runEvent }: { discussionId: string; runEvent?: RunEventHint }) {
  const { t } = useT();
  const [runs, setRuns] = useState<SharedRun[]>([]);
  const knownRunIds = useRef<Set<string>>(new Set());
  useEffect(() => { knownRunIds.current = new Set(runs.map(run => run.id)); }, [runs]);

  const reload = useCallback(() => {
    runsApi
      .list({ discussionId, limit: 20 })
      .then(setRuns)
      .catch(() => {
        /* Transient list failure — individual cards still self-hydrate. */
      });
  }, [discussionId]);

  useEffect(() => {
    setRuns([]);
    reload();
  }, [discussionId, reload]);

  const debounceTimer = useRef<number | null>(null);
  const scheduleRelist = useCallback(() => {
    if (debounceTimer.current != null) return;
    debounceTimer.current = window.setTimeout(() => {
      debounceTimer.current = null;
      reload();
    }, RELIST_DEBOUNCE_MS);
  }, [reload]);
  useEffect(() => () => { if (debounceTimer.current != null) window.clearTimeout(debounceTimer.current); }, []);

  const lastSeenSeq = useRef(0);
  useEffect(() => {
    if (!runEvent || runEvent.seq === lastSeenSeq.current) return;
    lastSeenSeq.current = runEvent.seq;
    // A known run's own card already reflects this update — relisting here
    // would only duplicate that live logic. Only an unseen run_id (a brand
    // new run just attached to this discussion) needs the list refreshed.
    if (!knownRunIds.current.has(runEvent.runId)) scheduleRelist();
  }, [runEvent, scheduleRelist]);

  if (runs.length === 0) return null;

  return (
    <div className="disc-attached-runs" data-testid="disc-attached-runs">
      <span className="disc-attached-runs-heading">{t('run.attachedHeading')}</span>
      <div className="disc-attached-runs-list">
        {runs.map(run => (
          <RunStatusCard key={run.id} model={sharedRunStatusCardModel(run, 'live')} runId={run.id} compact />
        ))}
      </div>
    </div>
  );
}

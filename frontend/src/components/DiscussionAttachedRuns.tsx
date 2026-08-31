import { useCallback, useEffect, useRef, useState } from 'react';
import { runsApi } from '../lib/api';
import { useT } from '../lib/I18nContext';
import { RunStatusCard, type RunStatusCardModel } from './RunStatusCard';
import type { SharedRun } from '../types/generated';

function toCardModel(run: SharedRun): RunStatusCardModel {
  const result = run.result as
    | { progress?: { completed: number; total: number; current_label?: string | null } }
    | null;
  return {
    id: run.id,
    kind: run.kind,
    status: run.status,
    startedAt: run.started_at,
    finishedAt: run.finished_at,
    durationMs: run.duration_ms,
    progress: result?.progress
      ? { ...result.progress, currentLabel: result.progress.current_label }
      : null,
    result: run.result,
    diagnostic: run.diagnostic,
    freshness: 'live',
    href: run.discussion_id
      ? `/discussions/${run.discussion_id}`
      : run.kind === 'workflow'
        ? `/workflows/${run.source_id}?run=${run.id}`
        : `/workflows?kind=${run.kind}&source=${run.source_id}&run=${run.id}`,
  };
}

/**
 * A run started elsewhere (QP batch, QA/QE launch, Workflow) that gets
 * attached to this discussion via its `discussion_id` shows up here
 * automatically, through the same server model and card used everywhere
 * else — no discussion-specific rendering logic.
 *
 * `refreshToken` is bumped by the page's single `useWebSocket` subscription
 * on `shared_run_updated` — this component deliberately does not open its
 * own socket subscription, so a page with N discussions/cards still shares
 * exactly one live connection (no duplicated live logic, DoD #2/#6).
 */
export function DiscussionAttachedRuns({ discussionId, refreshToken }: { discussionId: string; refreshToken?: number }) {
  const { t } = useT();
  const [runs, setRuns] = useState<SharedRun[]>([]);

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

  const skipNextRefreshToken = useRef(true);
  useEffect(() => {
    // The mount effect above already reloads once for the initial
    // refreshToken value — only react to a real bump after that.
    if (skipNextRefreshToken.current) { skipNextRefreshToken.current = false; return; }
    reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refreshToken]);

  if (runs.length === 0) return null;

  return (
    <div className="disc-attached-runs" data-testid="disc-attached-runs">
      <span className="disc-attached-runs-heading">{t('run.attachedHeading')}</span>
      <div className="disc-attached-runs-list">
        {runs.map(run => (
          <RunStatusCard key={run.id} model={toCardModel(run)} runId={run.id} compact />
        ))}
      </div>
    </div>
  );
}

import { useMemo } from 'react';
import { pages as pagesApi } from '../lib/api';
import type { LivePageAction } from '../types/generated';
import { KronnActionCard, type KronnActionOperations } from './DiscussionActionCard';

const operations: KronnActionOperations<LivePageAction> = {
  get: actionId => pagesApi.getAction(actionId),
  cancel: actionId => pagesApi.cancelAction(actionId),
  launch: (actionId, request) => pagesApi.launchAction(actionId, request),
};

export interface LivePageActionCardProps {
  action: LivePageAction;
  bindings?: Record<string, string>;
  onChanged: (action: LivePageAction) => void;
  onOpenDiscussion: (discussionId: string) => void;
}

export function LivePageActionCard({
  action,
  bindings,
  onChanged,
  onOpenDiscussion,
}: LivePageActionCardProps) {
  // The sandbox may send a fresh object for the same selector after a Page
  // script re-renders. A stable, sorted copy avoids restarting card effects
  // solely because property insertion order changed.
  const stableBindings = useMemo(
    () => Object.fromEntries(Object.entries(bindings ?? {}).sort(([left], [right]) => left.localeCompare(right))),
    [bindings],
  );
  return (
    <KronnActionCard
      action={action}
      operations={operations}
      bindings={stableBindings}
      onChanged={onChanged}
      onOpenDiscussion={onOpenDiscussion}
      initiallyExpanded
      testIdPrefix="live-page-action"
    />
  );
}

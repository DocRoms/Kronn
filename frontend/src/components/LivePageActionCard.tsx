import { useMemo, type CSSProperties } from 'react';
import { pages as pagesApi } from '../lib/api';
import type { LivePageAction } from '../types/generated';
import './LivePageActionSkin.css';
import { KronnActionCard, type KronnActionOperations } from './DiscussionActionCard';

export interface LivePageActionCardProps {
  action: LivePageAction;
  /** The offer this card was opened from, to launch the same row again. */
  offer?: LivePageAction | null;
  bindings?: Record<string, string>;
  /** The Page's display text for those bindings. */
  bindingLabels?: Record<string, string>;
  /** Starting values drawn from the Page's data for the clicked row. */
  prefill?: Record<string, string>;
  /** The Page's validated skin (`--kac-*` custom properties); see lib/live-page-action-skin. */
  skin?: Record<string, string> | null;
  onChanged: (action: LivePageAction) => void;
  onOpenDiscussion: (discussionId: string) => void;
}

export function LivePageActionCard({
  action,
  offer,
  bindings,
  bindingLabels,
  prefill,
  skin,
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
  const operations = useMemo<KronnActionOperations<LivePageAction>>(() => ({
    get: actionId => pagesApi.getAction(actionId),
    cancel: actionId => pagesApi.cancelAction(actionId),
    launch: (actionId, request) => pagesApi.launchAction(actionId, request),
    ...(offer ? { relaunch: () => Promise.resolve(offer) } : {}),
  }), [offer]);
  const skinned = skin !== undefined && skin !== null && Object.keys(skin).length > 0;
  return (
    <div className="live-page-action-skin" data-skinned={skinned ? 'true' : undefined} style={skinned ? (skin as CSSProperties) : undefined}>
    <KronnActionCard
      action={action}
      operations={operations}
      bindings={stableBindings}
      bindingLabels={bindingLabels}
      prefill={prefill}
      onChanged={onChanged}
      onOpenDiscussion={onOpenDiscussion}
      initiallyExpanded
      testIdPrefix="live-page-action"
    />
    </div>
  );
}

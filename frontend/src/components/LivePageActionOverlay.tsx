import type { LivePageAction } from '../types/generated';
import type { LivePageActiveActionState } from '../hooks/useLivePageActions';
import { LivePageActionCard } from './LivePageActionCard';
import './LivePageActionOverlay.css';

export interface LivePageActionOverlayProps {
  active: LivePageActiveActionState | null;
  action: LivePageAction | null;
  onChanged: (action: LivePageAction) => void;
  onOpenDiscussion: (discussionId: string) => void;
}

/** Anchors the shared native action card at the exact click point inside a
 * Page's iframe shell, identically for the embedded viewer, the standalone
 * tab and every mosaic tile. */
export function LivePageActionOverlay({ active, action, onChanged, onOpenDiscussion }: LivePageActionOverlayProps) {
  if (!active || !action) return null;
  return (
    <div
      className="live-page-action-overlay"
      style={{
        top: Math.max(8, active.anchor.top + active.anchor.height + 6),
        left: Math.max(8, active.anchor.left),
      }}
    >
      <LivePageActionCard
        key={`${action.id}:${active.activation}`}
        action={action}
        bindings={active.bindings}
        onChanged={onChanged}
        onOpenDiscussion={onOpenDiscussion}
      />
    </div>
  );
}

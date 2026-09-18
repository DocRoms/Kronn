import type { LivePageAction } from '../types/generated';
import type { LivePageActiveActionState } from '../hooks/useLivePageActions';
import { LivePageActionCard } from './LivePageActionCard';
import './LivePageActionOverlay.css';

export interface LivePageActionOverlayProps {
  active: LivePageActiveActionState | null;
  action: LivePageAction | null;
  onChanged: (action: LivePageAction, activation: number) => void;
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
        // The anchor is a preference, not a position: the stylesheet clamps it
        // so a CTA near the right edge slides the card left instead of
        // narrowing it. Setting `left` directly here is what crushed the card,
        // because it co-constrained the box with the `right` that used to sit
        // in the CSS.
        ['--kr-action-anchor-left' as string]: `${Math.max(8, active.anchor.left)}px`,
      }}
    >
      {/* One card per click. Keyed on the activation alone, so the card
          survives its own id changing from the offer to its launch. */}
      <LivePageActionCard
        key={active.activation}
        action={action}
        bindings={active.bindings}
        onChanged={changed => onChanged(changed, active.activation)}
        onOpenDiscussion={onOpenDiscussion}
      />
    </div>
  );
}

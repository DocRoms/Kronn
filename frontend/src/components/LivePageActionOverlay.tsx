import { useCallback, useEffect } from 'react';
import { X } from 'lucide-react';
import type { LivePageAction } from '../types/generated';
import type { LivePageActiveActionState } from '../hooks/useLivePageActions';
import { useT } from '../lib/I18nContext';
import { LivePageActionCard } from './LivePageActionCard';
import './LivePageActionOverlay.css';

export interface LivePageActionOverlayProps {
  active: LivePageActiveActionState | null;
  action: LivePageAction | null;
  /** The Page's offer for this click, whatever the card currently shows. */
  offer?: LivePageAction | null;
  onChanged: (action: LivePageAction, activation: number) => void;
  onClose: () => void;
  onOpenDiscussion: (discussionId: string) => void;
}

/** Anchors the shared native action card at the exact click point inside a
 * Page's iframe shell, identically for the embedded viewer, the standalone
 * tab and every mosaic tile. */
export function LivePageActionOverlay({ active, action, offer, onChanged, onClose, onOpenDiscussion }: LivePageActionOverlayProps) {
  const { t } = useT();
  const activation = active?.activation ?? 0;
  // Stable per click: the card restarts its polling whenever this changes.
  const handleChanged = useCallback(
    (changed: LivePageAction) => onChanged(changed, activation),
    [activation, onChanged],
  );
  const open = Boolean(active && action);
  useEffect(() => {
    if (!open) return undefined;
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose, open]);

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
      <button
        type="button"
        className="live-page-action-overlay__close"
        onClick={onClose}
        aria-label={t('common.close')}
        title={t('common.close')}
        data-testid="page-action-overlay-close"
      >
        <X size={14} aria-hidden />
      </button>
      {/* One card per click. Keyed on the activation alone, so the card
          survives its own id changing from the offer to its launch. */}
      <LivePageActionCard
        key={active.activation}
        action={action}
        offer={offer}
        bindings={active.bindings}
        onChanged={handleChanged}
        onOpenDiscussion={onOpenDiscussion}
      />
    </div>
  );
}

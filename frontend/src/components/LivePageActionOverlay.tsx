import { useCallback, useEffect, useRef } from 'react';
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
  /** The card's measured height, so the Page can reserve exactly that much
   * room under its row. `null` once the card is gone. */
  onHeightChange?: (height: number | null) => void;
}

/**
 * Place the host action card in the row slot reported by the iframe bridge.
 * Updated anchors keep it aligned while the Page scrolls.
 */
export function LivePageActionOverlay({ active, action, offer, onChanged, onClose, onOpenDiscussion, onHeightChange }: LivePageActionOverlayProps) {
  const { t } = useT();
  const cardRef = useRef<HTMLDivElement | null>(null);
  const reportHeight = useRef(onHeightChange);
  useEffect(() => { reportHeight.current = onHeightChange; });
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

  // The card grows as its run advances (steps appear, a form opens), so the
  // reserved room has to follow it rather than be measured once.
  useEffect(() => {
    const element = cardRef.current;
    if (!open || !element) {
      reportHeight.current?.(null);
      return undefined;
    }
    const publish = () => reportHeight.current?.(Math.ceil(element.getBoundingClientRect().height));
    publish();
    const observer = new ResizeObserver(publish);
    observer.observe(element);
    return () => {
      observer.disconnect();
      reportHeight.current?.(null);
    };
  }, [open]);

  if (!active || !action) return null;
  const inSlot = active.anchor.slot === true;
  const previous = active.previous;
  return (
    <div
      ref={cardRef}
      className="live-page-action-overlay"
      style={{
        // Fill an existing slot; before it opens, place the card below the row.
        // Do not clamp vertically: the card must scroll with its row.
        top: inSlot ? active.anchor.top : active.anchor.top + active.anchor.height + 4,
        // CSS constrains both offsets at the viewport edge; direct width/left values
        // would conflict with its right-side constraint.
        ['--kr-action-anchor-left' as string]: `${Math.max(8, active.anchor.left)}px`,
        ['--kr-action-anchor-width' as string]: `${active.anchor.width}px`,
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
      {previous && !active.card && (
        <button
          type="button"
          className="live-page-action-overlay__previous"
          onClick={() => onChanged(previous, activation)}
          data-testid="page-action-overlay-previous"
        >
          {t('pages.action.previousLaunch', t(`disc.action.state.${previous.state}`))}
        </button>
      )}
      {/* One card per click. Keyed on the activation alone, so the card
          survives its own id changing from the offer to its launch. */}
      <LivePageActionCard
        key={active.activation}
        action={action}
        offer={offer}
        bindings={active.bindings}
        prefill={active.prefill}
        onChanged={handleChanged}
        onOpenDiscussion={onOpenDiscussion}
      />
    </div>
  );
}

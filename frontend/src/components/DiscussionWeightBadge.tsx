// Storage-weight indicator for one discussion card. All the behaviour lives
// here so the card itself stays free of weight logic.
//
// The detail panel goes through a portal: the card clips its content and
// carries a swipe handler, so an inline popover would be cut off or swallowed.
import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { DiscussionWeightView, WeightLevel } from '../types/generated';
import { formatBytes, type WeightLoadState } from '../lib/weightFormat';
import './DiscussionWeightBadge.css';

type T = (key: string, ...args: (string | number)[]) => string;


const LEVEL_ORDER: Record<WeightLevel, number> = { green: 0, amber: 1, red: 2 };

/** Space between the badge and its panel. */
const GAP = 6;
/** Keeps the panel off the very edge of the window. */
const VIEWPORT_MARGIN = 8;


export function DiscussionWeightBadge({
  weight,
  state,
  t,
}: {
  weight?: DiscussionWeightView | null;
  state: WeightLoadState;
  t: T;
}) {
  const [open, setOpen] = useState(false);
  const [anchor, setAnchor] = useState<{ top: number; left: number } | null>(null);
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const panelId = useId();

  const close = useCallback(() => {
    setOpen(false);
    buttonRef.current?.focus();
  }, []);

  // Sidebar rows sit at any height and near the right edge, so the panel is
  // measured once mounted and pulled back inside the viewport — flipped above
  // the badge when there is no room below.
  useEffect(() => {
    if (!open || !anchor) return;
    const panel = panelRef.current;
    const button = buttonRef.current;
    if (!panel || !button) return;
    const panelBox = panel.getBoundingClientRect();
    const buttonBox = button.getBoundingClientRect();
    const maxLeft = window.innerWidth - panelBox.width - VIEWPORT_MARGIN;
    const left = Math.max(VIEWPORT_MARGIN, Math.min(anchor.left, maxLeft));
    const overflowsBelow = anchor.top + panelBox.height + VIEWPORT_MARGIN > window.innerHeight;
    const flipped = buttonBox.top - panelBox.height - GAP;
    const top = overflowsBelow && flipped >= VIEWPORT_MARGIN ? flipped : anchor.top;
    if (Math.abs(left - anchor.left) > 0.5 || Math.abs(top - anchor.top) > 0.5) {
      setAnchor({ top, left });
    }
  }, [open, anchor]);

  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        close();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [open, close]);

  // Anything other than `ready` shows NO number. A placeholder zero reads as
  // "this discussion is empty", which is a different fact entirely — and for
  // `unmeasured` (a row outside the bounded batch) it would be an invented
  // measurement.
  if (state !== 'ready') {
    const label =
      state === 'loading'
        ? t('disc.weight.loading')
        : state === 'unmeasured'
          ? t('disc.weight.unmeasured')
          : t('disc.weight.unavailable');
    return (
      <span
        className="disc-weight-badge disc-weight-badge--pending"
        data-testid="disc-weight-pending"
        data-state={state}
        aria-label={label}
        title={label}
      >
        <span className="disc-weight-dot" aria-hidden="true" />
      </span>
    );
  }

  const total = weight?.total_bytes ?? 0;
  const level: WeightLevel = weight?.level ?? 'green';
  const reclaimable = weight?.reclaimable_bytes ?? 0;
  const disk = weight?.disk_bytes ?? 0;
  const extracted = weight?.extracted_text_bytes ?? 0;
  const messages = weight?.message_bytes ?? 0;

  const openPanel = () => {
    const rect = buttonRef.current?.getBoundingClientRect();
    if (rect) setAnchor({ top: rect.bottom + GAP, left: rect.left });
    setOpen(true);
  };

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        className="disc-weight-badge"
        data-testid="disc-weight-badge"
        data-level={level}
        data-severity={LEVEL_ORDER[level]}
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        aria-label={t('disc.weight.badgeLabel', formatBytes(total))}
        onClick={event => {
          // The card is a button and carries a swipe handler; a bubbling
          // click would select the discussion instead of opening the detail.
          event.stopPropagation();
          event.preventDefault();
          if (open) {
            close();
          } else {
            openPanel();
          }
        }}
        onKeyDown={event => {
          if (event.key === 'Enter' || event.key === ' ') event.stopPropagation();
        }}
      >
        <span className="disc-weight-dot" aria-hidden="true" />
        <span className="disc-weight-value">{formatBytes(total)}</span>
      </button>

      {open && anchor && createPortal(
        <>
          <div className="disc-weight-panel-backdrop" onClick={close} />
          <div
            id={panelId}
            role="dialog"
            aria-modal="false"
            aria-label={t('disc.weight.detailTitle')}
            ref={panelRef}
            className="disc-weight-panel"
            data-testid="disc-weight-panel"
            style={{ top: anchor.top, left: anchor.left }}
            onClick={event => event.stopPropagation()}
          >
            <div className="disc-weight-panel-total" data-level={level}>
              {t('disc.weight.total', formatBytes(total))}
            </div>
            <dl className="disc-weight-panel-rows">
              <div>
                <dt>{t('disc.weight.disk')}</dt>
                <dd>{formatBytes(disk)}</dd>
              </div>
              <div>
                <dt>{t('disc.weight.extracted')}</dt>
                <dd>{formatBytes(extracted)}</dd>
              </div>
              <div>
                <dt>{t('disc.weight.messages')}</dt>
                <dd>{formatBytes(messages)}</dd>
              </div>
            </dl>
            <p className="disc-weight-panel-hint" data-testid="disc-weight-reclaimable">
              {t('disc.weight.reclaimable', formatBytes(reclaimable))}
            </p>
          </div>
        </>,
        document.body,
      )}
    </>
  );
}

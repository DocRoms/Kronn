// KT-1344 — the header shell the discussion and the artifact pages share.
//
// Three rows, in the same place on every page that has a detail view:
//   1. the title, with ONE action pinned to its right (the discussion's invite,
//      the artifact's sync — the control that opens something about the whole
//      object);
//   2. what the object is and the actions on it, with the Details toggle at the
//      right end, under the pinned action;
//   3. the fold: figures and badges a reader consults, not controls they act on.
//
// The fold's open/closed state is remembered per page type (`storageKey`), so
// someone who wants the third row permanently opens it once on that page.
//
// A page whose details already have a home in a side panel (the discussion)
// passes `onOpenDetails` instead: the toggle becomes a button that opens that
// panel, and there is no third row.
import { useState, type ReactNode } from 'react';
import { ChevronDown, PanelRightOpen } from 'lucide-react';
import '../pages/DiscussionsPage.css';

type DataAttrs = { 'data-tour-id'?: string };

export interface DetailHeaderShellProps {
  /** Page-type key: the fold is remembered under it. Not needed with `onOpenDetails`. */
  storageKey?: string;
  /** Anchors the toggle's `aria-controls` and the fold's `id`. */
  detailsId: string;
  /** Button label, and the title for each state. */
  detailsLabel: string;
  detailsShowTitle: string;
  detailsHideTitle: string;
  toggleTestId?: string;
  /** Opens the page's side panel on its details instead of unfolding a row:
   *  the button then carries the panel icon and `details` is not rendered. */
  onOpenDetails?: () => void;
  /** Whether that panel is showing, for `aria-expanded`. */
  detailsPanelOpen?: boolean;
  /** Extra class on the root, for page-specific bleed/spacing. */
  className?: string;
  /** Attributes for the root and for the title block (the tour anchors). */
  rootProps?: DataAttrs;
  titleProps?: DataAttrs;
  /** Before the info column (the mobile sidebar button). */
  leading?: ReactNode;
  title: ReactNode;
  /** The pinned slot on the title row, right side. */
  trailing?: ReactNode;
  /** Row 2, before the Details toggle. */
  sub: ReactNode;
  /** Row 3, rendered only while the fold is open. */
  details?: ReactNode;
  /** After the info column (the discussion's output controls). */
  actions?: ReactNode;
}

function readDetailsOpen(storageKey: string | undefined): boolean {
  if (!storageKey) return false;
  try {
    return localStorage.getItem(storageKey) === '1';
  } catch {
    return false;
  }
}

export function DetailHeaderShell({
  storageKey, detailsId, detailsLabel, detailsShowTitle, detailsHideTitle, toggleTestId,
  onOpenDetails, detailsPanelOpen, className, rootProps, titleProps, leading, title, trailing, sub, details, actions,
}: DetailHeaderShellProps) {
  const [detailsOpen, setDetailsOpen] = useState(() => readDetailsOpen(storageKey));

  const toggleDetails = () => {
    setDetailsOpen(open => {
      const next = !open;
      try {
        if (storageKey) localStorage.setItem(storageKey, next ? '1' : '0');
      } catch {
        // A browser refusing storage still gets the toggle, just not the memory.
      }
      return next;
    });
  };

  return (
    <div
      className={`disc-chat-header collection-detail-header${className ? ` ${className}` : ''}`}
      {...rootProps}
    >
      {leading}
      <div className="disc-chat-header-info">
        <div className="disc-chat-header-top">
          <div className="disc-chat-header-title" {...titleProps}>
            {title}
          </div>
          {trailing !== undefined && (
            <div className="disc-chat-header-invite">{trailing}</div>
          )}
        </div>
        <div className="disc-chat-header-sub">
          {sub}
          {onOpenDetails ? (
            <button
              type="button"
              className="disc-chat-header-details-btn"
              data-testid={toggleTestId}
              onClick={onOpenDetails}
              aria-expanded={detailsPanelOpen ?? false}
              aria-controls={detailsId}
              title={detailsPanelOpen ? detailsHideTitle : detailsShowTitle}
            >
              <PanelRightOpen size={13} />
              <span>{detailsLabel}</span>
            </button>
          ) : (
            <button
              type="button"
              className="disc-chat-header-details-toggle"
              data-testid={toggleTestId}
              onClick={toggleDetails}
              aria-expanded={detailsOpen}
              aria-controls={detailsId}
              title={detailsOpen ? detailsHideTitle : detailsShowTitle}
            >
              <ChevronDown size={11} />
              <span>{detailsLabel}</span>
            </button>
          )}
        </div>
        {!onOpenDetails && detailsOpen && (
          <div className="disc-chat-header-details" id={detailsId}>
            {details}
          </div>
        )}
      </div>
      {actions}
    </div>
  );
}

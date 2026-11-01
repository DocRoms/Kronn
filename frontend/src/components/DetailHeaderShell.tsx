// KT-1344 / KT-1530 — the header shell the discussion and the artifact pages share.
//
// Two rows, in the same place on every page that has a detail view:
//   1. the title, the object's copyable id right next to it, and at the far
//      right the same pair of buttons everywhere: the page's primary action
//      (the discussion's invite, the artifact's sync — green) and "Details";
//   2. what the object is and the actions on it.
//
// Anything else that belongs with the buttons (`status`) goes BEFORE the pair:
// "Details" is the rightmost control of every header, at the same right edge.
//
// "Details" always opens the page's vertical side panel on its details tab:
// there is no folded third row. The id is NOT part of those details — people
// click it all the time to mention the object to an agent, so it stays on the
// title row (`idPill`).
import type { ReactNode, Ref } from 'react';
import { PanelRightOpen } from 'lucide-react';
import '../pages/DiscussionsPage.css';

type DataAttrs = { 'data-tour-id'?: string };

export interface DetailHeaderShellProps {
  /** Anchors the Details button's `aria-controls` (the side panel's id). */
  detailsId: string;
  /** Button label, and the title for each state. */
  detailsLabel: string;
  detailsShowTitle: string;
  detailsHideTitle: string;
  toggleTestId?: string;
  /** Lets the page return focus to the Details button when its panel closes. */
  detailsButtonRef?: Ref<HTMLButtonElement>;
  /** Opens (or closes) the page's side panel on its details tab. */
  onOpenDetails: () => void;
  /** Whether that panel is showing on the details tab, for `aria-expanded`. */
  detailsPanelOpen?: boolean;
  /** Extra class on the root, for page-specific bleed/spacing. */
  className?: string;
  /** Attributes for the root and for the title block (the tour anchors). */
  rootProps?: DataAttrs;
  titleProps?: DataAttrs;
  /** Before the info column (the mobile sidebar button). */
  leading?: ReactNode;
  title: ReactNode;
  /** The copyable id, directly after the title. */
  idPill?: ReactNode;
  /** Status that belongs with the buttons (the pending-learnings badge, the sync
   *  freshness): in the same group and on the same baseline, but BEFORE the pair,
   *  so « Details » stays the rightmost control of the header. */
  status?: ReactNode;
  /** The page's primary action, left of Details. Green, same size as Details. */
  primaryAction?: ReactNode;
  /** Attributes for the button group (the tour anchor). */
  buttonsProps?: DataAttrs;
  /** Row 2. */
  sub: ReactNode;
  /** After the info column (the discussion's output controls). */
  actions?: ReactNode;
}

export function DetailHeaderShell({
  detailsId, detailsLabel, detailsShowTitle, detailsHideTitle, toggleTestId, detailsButtonRef,
  onOpenDetails, detailsPanelOpen, className, rootProps, titleProps, leading, title, idPill,
  status, primaryAction, buttonsProps, sub, actions,
}: DetailHeaderShellProps) {
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
            {idPill}
          </div>
          <div className="disc-chat-header-buttons" data-testid="detail-header-buttons" {...buttonsProps}>
            {status}
            {primaryAction}
            <button
              type="button"
              className="disc-chat-header-btn disc-chat-header-details-btn"
              data-testid={toggleTestId}
              ref={detailsButtonRef}
              onClick={onOpenDetails}
              aria-expanded={detailsPanelOpen ?? false}
              aria-controls={detailsId}
              title={detailsPanelOpen ? detailsHideTitle : detailsShowTitle}
            >
              <PanelRightOpen size={13} />
              <span>{detailsLabel}</span>
            </button>
          </div>
        </div>
        <div className="disc-chat-header-sub">
          {sub}
        </div>
      </div>
      {actions}
    </div>
  );
}

// KT-581 — switching panels, next to the panel being switched.
//
// Six panels live on a discussion. They used to be six buttons in the header
// row, which also carries the title, the agent, the tier and the counters, and
// every version narrowed it further.
//
// A floating list fixed the width and created a worse problem: each panel
// already draws its own header with its own icons, so the list showed the same
// icon twice — once in the list, once in the open panel — and it hung under
// the header, unattached to the thing it controlled.
//
// So this strip lives in the panel's own column, above the panel, and it also
// carries the control that opens the column at all. Left in the discussion
// header, that control was separated from its panel by a whole row of
// counters — the two read as unrelated things.
import { PanelRightClose, PanelRightOpen } from 'lucide-react';
import './DiscussionPanelSwitcher.css';

export interface SwitchablePanel {
  id: string;
  label: string;
  icon: React.ReactNode;
  active: boolean;
  onSelect: () => void;
  /** Pending count, when the panel has one — files to commit, proposals… */
  badge?: React.ReactNode;
}

export function DiscussionPanelSwitcher({
  panels,
  actions = [],
  leading,
  groupLabel,
  openLabel,
  closeLabel,
  onToggleColumn,
}: {
  panels: SwitchablePanel[];
  /** Things the discussion does rather than shows — export, delete. They sit
   *  after the panels, behind a separator, so a destructive one is never the
   *  neighbour of a panel someone meant to click. */
  actions?: SwitchablePanel[];
  /** Rendered right after the open control, and always visible. Message
   *  search lives here: it is not a panel, but it belongs to the same strip
   *  so the header keeps nothing. */
  leading?: React.ReactNode;
  groupLabel: string;
  openLabel: string;
  closeLabel: string;
  /** Opens the column on the last-read panel, or closes whatever is open. */
  onToggleColumn: () => void;
}) {
  const anyOpen = panels.some(panel => panel.active);

  // Always rendered, and always in the panel's own column: the control that
  // opens a panel has to sit against it. Left in the discussion header it was
  // separated from the panel by a whole row of counters, so the two read as
  // unrelated things.
  return (
    <div
      className="disc-panel-switcher"
      role="group"
      aria-label={groupLabel}
      data-testid="panel-switcher"
      data-open={anyOpen}
    >
      <button
        type="button"
        className="disc-panel-switcher-item disc-panel-switcher-toggle"
        data-active={anyOpen}
        onClick={onToggleColumn}
        title={anyOpen ? closeLabel : openLabel}
        aria-label={anyOpen ? closeLabel : openLabel}
        aria-expanded={anyOpen}
        data-testid="panel-open-toggle"
      >
        {anyOpen ? <PanelRightClose size={14} /> : <PanelRightOpen size={14} />}
      </button>
      {/* After the open control, not before: the first button in the strip is
       *  the one that opens it, and search is a companion to that. */}
      {leading}
      {/* The panel icons only make sense once there is a panel to move away
       *  from; before that the column stays a narrow strip. */}
      {anyOpen && panels.map(panel => (
        <button
          key={panel.id}
          type="button"
          className="disc-panel-switcher-item"
          data-active={panel.active}
          data-panel={panel.id}
          onClick={panel.onSelect}
          // These reveal a region rather than toggling a setting, so a screen
          // reader should say "collapsed", not "not pressed".
          aria-expanded={panel.active}
          title={panel.label}
          aria-label={panel.label}
        >
          {panel.icon}
          {panel.badge != null && (
            <span className="disc-panel-switcher-badge" aria-hidden="true">{panel.badge}</span>
          )}
        </button>
      ))}
      {anyOpen && actions.length > 0 && (
        <>
          <span className="disc-panel-switcher-sep" aria-hidden="true" />
          {actions.map(action => (
            <button
              key={action.id}
              type="button"
              className="disc-panel-switcher-item"
              data-action={action.id}
              onClick={action.onSelect}
              title={action.label}
              aria-label={action.label}
            >
              {action.icon}
            </button>
          ))}
        </>
      )}
    </div>
  );
}

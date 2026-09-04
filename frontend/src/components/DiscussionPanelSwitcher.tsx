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
// So the switcher lives WITH the panel: it appears above it when one is open,
// and disappears with it. Nothing is duplicated, and the control sits where
// its effect is visible.
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
  groupLabel,
}: {
  panels: SwitchablePanel[];
  groupLabel: string;
}) {
  // Nothing open means nothing to switch between: the header's single control
  // is what opens the first one.
  if (!panels.some(panel => panel.active)) return null;

  return (
    <div
      className="disc-panel-switcher"
      role="group"
      aria-label={groupLabel}
      data-testid="panel-switcher"
    >
      {panels.map(panel => (
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
    </div>
  );
}

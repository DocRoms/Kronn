// KT-581 — the panels moved out of the header row.
//
// Six of them live on a discussion: plan, assets, git, terminal, settings and
// message search. Each version added one to a row that also carries the title,
// the agent, the tier and the counters, and the row got narrower every time.
// The Notes panel would have been the seventh.
//
// So the panels are listed once, in a rail that unfolds, and the header keeps
// a single control. Adding one widens nothing: it becomes another line in a
// list that already scrolls.
import { useEffect, useRef, useState } from 'react';
import { PanelLeftClose, PanelLeftOpen } from 'lucide-react';
import './DiscussionPanelRail.css';

export interface RailPanel {
  id: string;
  label: string;
  icon: React.ReactNode;
  /** Open right now. Panels are mutually exclusive, so at most one is true. */
  active: boolean;
  onToggle: () => void;
  /** Small count shown next to the label — pending files, asset count… */
  badge?: React.ReactNode;
  /** Extra trailing content, e.g. the plan's completed/total pair. */
  trailing?: React.ReactNode;
}

export function DiscussionPanelRail({
  panels,
  expandLabel,
  collapseLabel,
  groupLabel,
}: {
  /** In the order they should be read. The first one is what opens by habit. */
  panels: RailPanel[];
  expandLabel: string;
  collapseLabel: string;
  groupLabel: string;
}) {
  // Folded by default, and remembered: someone who works with the rail open
  // should not re-open it at every visit, and someone who never uses it
  // should not carry it.
  const [expanded, setExpanded] = useState(() => {
    try {
      return localStorage.getItem('kronn:panelRailExpanded') === 'true';
    } catch {
      return false;
    }
  });
  const railRef = useRef<HTMLDivElement>(null);

  const setAndRemember = (next: boolean) => {
    setExpanded(next);
    try {
      localStorage.setItem('kronn:panelRailExpanded', String(next));
    } catch {
      // A storage failure must never prevent opening a panel.
    }
  };

  // Escape folds the rail rather than closing the panel behind it: the reader
  // asked to put the list away, not to lose their place.
  useEffect(() => {
    if (!expanded) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setAndRemember(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [expanded]);

  const openPanel = panels.find(panel => panel.active);
  // Folding the rail must not hide that something is waiting. The header used
  // to show these counts directly — pending files, pending proposals — and a
  // reader who glances at it should still learn there is something to open.
  const foldedBadges = panels.filter(panel => panel.badge != null);

  return (
    <div className="disc-panel-rail" ref={railRef} data-expanded={expanded}>
      <button
        type="button"
        className="disc-icon-btn disc-panel-rail-toggle"
        onClick={() => setAndRemember(!expanded)}
        title={expanded ? collapseLabel : expandLabel}
        aria-label={expanded ? collapseLabel : expandLabel}
        aria-expanded={expanded}
        data-testid="panel-rail-toggle"
        // The header must still say WHICH panel is open once the rail is
        // folded, or folding it hides the only clue.
        data-open-panel={openPanel?.id}
      >
        {expanded ? <PanelLeftClose size={13} /> : <PanelLeftOpen size={13} />}
        {!expanded && openPanel && (
          <span className="disc-panel-rail-current">{openPanel.label}</span>
        )}
        {!expanded && foldedBadges.length > 0 && (
          <span
            className="disc-panel-rail-dot"
            data-testid="panel-rail-attention"
            title={foldedBadges.map(panel => panel.label).join(' · ')}
            aria-label={foldedBadges.map(panel => panel.label).join(' · ')}
          >
            {foldedBadges.length}
          </span>
        )}
      </button>

      {expanded && (
        <div
          className="disc-panel-rail-list"
          role="group"
          aria-label={groupLabel}
          data-testid="panel-rail-list"
        >
          {panels.map(panel => (
            <button
              key={panel.id}
              type="button"
              className="disc-panel-rail-item"
              data-active={panel.active}
              data-panel={panel.id}
              onClick={panel.onToggle}
              // `expanded`, not `pressed`: these open a region, they are not
              // on/off switches. The distinction is what a screen reader uses
              // to say "collapsed" instead of "not pressed".
              aria-expanded={panel.active}
              title={panel.label}
            >
              <span className="disc-panel-rail-icon" aria-hidden="true">{panel.icon}</span>
              {/* The label is always rendered, never hover-only: an icon rail
               *  nobody can read is a worse header row, not a better one. */}
              <span className="disc-panel-rail-label">{panel.label}</span>
              {panel.trailing}
              {panel.badge != null && (
                <span className="disc-panel-rail-badge">{panel.badge}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

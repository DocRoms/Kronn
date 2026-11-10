import { useRef, useState } from 'react';
import { Bell } from 'lucide-react';
import { useActivityOverview } from '../../hooks/useActivityOverview';
import { useT } from '../../lib/I18nContext';
import {
  ACTIVITY_ACTION_HANDLERS,
  isAnythingRunning,
  waitingOnOwnerCount,
  type ActivityActionHandler,
} from '../../lib/activity';
import type { ActivityLink } from '../../types/generated';
import { ActivityPanel } from './ActivityPanel';
import './ActivityBell.css';

export interface ActivityBellProps {
  /** Opens the place a row points at. The panel closes first. */
  onOpenLink: (link: ActivityLink) => void;
  /** An action settled: the host refreshes whatever else shows that state. */
  onActionSettled?: () => void;
  handlers?: Readonly<Record<string, ActivityActionHandler>>;
}

/**
 * The one permanent entry to everything running in Kronn (KT-1658). It is
 * always in the header, moves while something runs, and carries a counter when
 * something waits for the owner. It owns its polling, so a refresh of the
 * activity re-renders the bell and its panel, not the whole dashboard.
 */
export function ActivityBell({ onOpenLink, onActionSettled, handlers = ACTIVITY_ACTION_HANDLERS }: ActivityBellProps) {
  const { t } = useT();
  const { overview, failed, refetch } = useActivityOverview();
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);

  const running = isAnythingRunning(overview);
  const waiting = waitingOnOwnerCount(overview);
  const inProgress = (overview?.rows ?? []).filter(row => row.compartment === 'running').length;

  const label = waiting > 0
    ? t('activity.bell.labelWaiting', waiting, inProgress)
    : inProgress > 0
      ? t('activity.bell.labelRunning', inProgress)
      : t('activity.bell.label');

  const toggle = () => {
    // Opening shows the current state, not the one of the last poll.
    if (!open) void refetch();
    setOpen(!open);
  };

  return (
    <div className="dash-nav-activity-wrap">
      <button
        ref={triggerRef}
        type="button"
        className="dash-nav-activity-btn activity-bell"
        data-testid="activity-bell"
        data-running={running ? 'true' : 'false'}
        aria-label={label}
        title={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? 'activity-panel' : undefined}
        onClick={toggle}
      >
        <Bell size={14} className="activity-bell-icon" aria-hidden="true" />
        {waiting > 0 && (
          <span className="activity-bell-badge" data-testid="activity-bell-badge" aria-hidden="true">
            {waiting > 99 ? '99+' : waiting}
          </span>
        )}
      </button>
      {open && (
        <ActivityPanel
          overview={overview}
          failed={failed}
          triggerRef={triggerRef}
          handlers={handlers}
          onClose={() => setOpen(false)}
          onOpenLink={link => {
            setOpen(false);
            onOpenLink(link);
          }}
          onActionSettled={() => {
            void refetch();
            onActionSettled?.();
          }}
        />
      )}
      {/* The state change is announced without moving focus. */}
      <span className="activity-bell-live" role="status" aria-live="polite" data-testid="activity-bell-live">
        {waiting > 0 ? t('activity.bell.labelWaiting', waiting, inProgress) : ''}
      </span>
    </div>
  );
}

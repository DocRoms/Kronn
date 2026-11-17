import { useEffect, useRef, useState } from 'react';
import { Bell } from 'lucide-react';
import { useActivityOverview } from '../../hooks/useActivityOverview';
import { useSecondTicker } from '../../hooks/useSecondTicker';
import { useT } from '../../lib/I18nContext';
import {
  ACTIVITY_ACTION_HANDLERS,
  isAnythingRunning,
  waitingOnOwnerCount,
  type ActivityActionHandler,
} from '../../lib/activity';
import type { ActivityTab } from '../../lib/activityFilters';
import { isUrgentNotice, unreadCount, useNotifications } from '../../lib/notifications';
import type { ActivityLink } from '../../types/generated';
import { ActivityPanel } from './ActivityPanel';
import './ActivityBell.css';

/** An outage that lasts this long opens the panel on its own, on Notifications. */
export const OUTAGE_AUTO_OPEN_MS = 10_000;

export interface ActivityBellProps {
  /** Opens the place a row points at. The panel closes first. */
  onOpenLink: (link: ActivityLink) => void;
  /** An action settled: the host refreshes whatever else shows that state. */
  onActionSettled?: () => void;
  handlers?: Readonly<Record<string, ActivityActionHandler>>;
}

/**
 * The one permanent entry to everything running in Kronn (KT-1658) and, since
 * KT-1684, to what went wrong: it sits at the right of the header, turns red
 * with a "!" while the backend is unreachable, and carries an orange dot for
 * notifications nobody read yet. It owns its polling, so a refresh of the
 * activity re-renders the bell and its panel, not the whole dashboard.
 */
export function ActivityBell({ onOpenLink, onActionSettled, handlers = ACTIVITY_ACTION_HANDLERS }: ActivityBellProps) {
  const { t } = useT();
  const { overview, failed, updatedAt, refetch } = useActivityOverview();
  const notifications = useNotifications();
  const [open, setOpen] = useState(false);
  const [openTab, setOpenTab] = useState<ActivityTab | undefined>(undefined);
  const triggerRef = useRef<HTMLButtonElement>(null);
  // `since` of the outage whose panel opened by itself: it closes by itself too,
  // unless the owner took the panel over in between.
  const autoOpened = useRef<number | null>(null);
  const autoOpenHandled = useRef<number | null>(null);

  const outage = notifications.outage;
  // The shared clock only wakes the bell each second while an outage lasts; the
  // elapsed time itself is read from the real clock.
  const tickedAt = useSecondTicker(outage !== null);
  const unread = unreadCount(notifications);

  const running = isAnythingRunning(overview);
  const waiting = waitingOnOwnerCount(overview);
  const inProgress = (overview?.rows ?? []).filter(row => row.compartment === 'running').length;

  useEffect(() => {
    if (outage && Date.now() - outage.since >= OUTAGE_AUTO_OPEN_MS && autoOpenHandled.current !== outage.since) {
      autoOpenHandled.current = outage.since;
      if (!open) {
        autoOpened.current = outage.since;
        // eslint-disable-next-line react-hooks/set-state-in-effect -- opening follows a clock threshold on external store state, there is no event to hang it on.
        setOpenTab('notifications');
        setOpen(true);
      }
    }
    if (!outage && autoOpened.current !== null) {
      autoOpened.current = null;
      setOpen(false);
    }
  }, [outage, tickedAt, open]);

  // A notice that stops work, or says an action failed, opens the panel once:
  // the banner it replaces was on screen at once.
  const urgentOpened = useRef(new Set<string>());
  useEffect(() => {
    const urgent = notifications.notices.filter(notice => isUrgentNotice(notice) && !notice.seen);
    const fresh = urgent.filter(notice => !urgentOpened.current.has(notice.id));
    urgentOpened.current = new Set(urgent.map(notice => notice.id));
    if (fresh.length === 0 || open) return;
    // eslint-disable-next-line react-hooks/set-state-in-effect -- opening follows external store state, there is no event to hang it on.
    setOpenTab('notifications');
    setOpen(true);
  }, [notifications.notices, open]);

  const activityLabel = waiting > 0
    ? t('activity.bell.labelWaiting', waiting, inProgress)
    : inProgress > 0
      ? t('activity.bell.labelRunning', inProgress)
      : t('activity.bell.label');
  const label = outage
    ? t('notif.bell.labelOutage')
    : unread > 0
      ? `${activityLabel} — ${t('notif.bell.labelUnread', unread)}`
      : activityLabel;

  const closePanel = () => {
    autoOpened.current = null;
    setOpen(false);
  };

  const toggle = () => {
    autoOpened.current = null;
    // Opening shows the current state, not the one of the last poll.
    if (!open) {
      void refetch();
      setOpenTab(undefined);
    }
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
        data-alert={outage ? 'outage' : undefined}
        data-unread={!outage && unread > 0 ? 'true' : undefined}
        aria-label={label}
        title={label}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-controls={open ? 'activity-panel' : undefined}
        onClick={toggle}
      >
        <Bell size={14} className="activity-bell-icon" aria-hidden="true" />
        {outage ? (
          <span className="activity-bell-bang" data-testid="activity-bell-alert" aria-hidden="true">!</span>
        ) : (
          <>
            {waiting > 0 && (
              <span className="activity-bell-badge" data-testid="activity-bell-badge" aria-hidden="true">
                {waiting > 99 ? '99+' : waiting}
              </span>
            )}
            {unread > 0 && (
              <span className="activity-bell-dot" data-testid="activity-bell-dot" aria-hidden="true" />
            )}
          </>
        )}
      </button>
      {open && (
        <ActivityPanel
          overview={overview}
          failed={failed}
          initialTab={openTab}
          asOf={updatedAt}
          triggerRef={triggerRef}
          handlers={handlers}
          onClose={closePanel}
          onOpenLink={link => {
            closePanel();
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
        {outage
          ? t('notif.bell.labelOutage')
          : [waiting > 0 ? t('activity.bell.labelWaiting', waiting, inProgress) : '', unread > 0 ? t('notif.bell.labelUnread', unread) : '']
            .filter(Boolean).join(' — ')}
      </span>
    </div>
  );
}

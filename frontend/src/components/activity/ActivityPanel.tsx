import { useEffect, useRef, useState, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { Clock, Loader2, Square, TriangleAlert, RotateCcw, X, XCircle } from 'lucide-react';
import { newTabClickProps } from '../../lib/newTabNavigation';
import { useActivityPopover } from '../../hooks/useActivityPopover';
import { useT } from '../../lib/I18nContext';
import {
  ACTIVITY_ACTION_HANDLERS,
  actionLabelKey,
  activityLinkPath,
  formatDuration,
  formatTimeOfDay,
  humanizeType,
  sortRowsForPanel,
  translateOr,
  type ActivityActionHandler,
} from '../../lib/activity';
import {
  ACTIVITY_CATEGORIES,
  categoryOfRow,
  countByCategory,
  readFilter,
  writeFilter,
  type ActivityFilter,
  type ActivityTab,
} from '../../lib/activityFilters';
import {
  hasUnresolvedAlert,
  markNotificationsSeen,
  unreadCount,
  useNotifications,
} from '../../lib/notifications';
import { ActivityFilterChips } from './ActivityFilterChips';
import { NotificationsTab } from './NotificationsTab';
import type { ActivityAction, ActivityLink, ActivityOverview, ActivityRow } from '../../types/generated';
import './ActivityPanel.css';

export interface ActivityPanelProps {
  overview: ActivityOverview | null;
  /** The last poll failed: what is shown may be out of date. */
  failed?: boolean;
  /** The tab to open on; by default Notifications when an alert is unresolved, else In progress. */
  initialTab?: ActivityTab;
  /** When the overview shown was last read (ms): the "as of" of a greyed view. */
  asOf?: number | null;
  onClose: () => void;
  onOpenLink: (link: ActivityLink) => void;
  /** Called once an action has settled, whichever way, so the view refreshes. */
  onActionSettled?: () => void;
  /** The call behind each action id; the app's own by default. */
  handlers?: Readonly<Record<string, ActivityActionHandler>>;
  triggerRef?: RefObject<HTMLButtonElement | null>;
  focusFallbackRef?: RefObject<HTMLElement | null>;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

const ACTION_ICON = {
  stop: <Square size={10} style={{ fill: 'currentColor' }} aria-hidden="true" />,
  cancel: <XCircle size={11} aria-hidden="true" />,
  resume_now: <RotateCcw size={10} aria-hidden="true" />,
} as const;

/**
 * The attention center's panel (KT-1658), in two tabs since KT-1684: In
 * progress, and Notifications (the outage and the server errors). Each tab has
 * the same category chips; "All" groups the list into one titled section per
 * category. The In progress side knows the generic row facts — title, state,
 * elapsed time, a stalled hint, a link, actions — and nothing about what a row
 * is, so a new row type needs no change here: its compartment only orders it
 * (the KT-334 ones, `needs_decision` and `unseen_done`, are tagged on the row).
 */
export function ActivityPanel({
  overview,
  failed = false,
  initialTab,
  asOf = null,
  onClose,
  onOpenLink,
  onActionSettled,
  handlers = ACTIVITY_ACTION_HANDLERS,
  triggerRef,
  focusFallbackRef,
}: ActivityPanelProps) {
  const { t } = useT();
  const { rootRef, closeAndRestoreFocus } = useActivityPopover({ onClose, triggerRef, focusFallbackRef });
  const [now, setNow] = useState(() => Date.now());
  // A synchronous per-action guard as well as the disabled button: two clicks
  // in one frame must not stop or resume twice.
  const pending = useRef(new Set<string>());
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const [errors, setErrors] = useState<Readonly<Record<string, string>>>({});

  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const perform = async (row: ActivityRow, action: ActivityAction) => {
    const handler = handlers[action.id];
    const key = `${row.id}|${action.id}`;
    if (!handler || pending.current.has(key)) return;
    pending.current.add(key);
    setBusy(new Set(pending.current));
    setErrors(current => {
      const rest = { ...current };
      delete rest[row.id];
      return rest;
    });
    try {
      await handler(action.args);
    } catch (error) {
      setErrors(current => ({ ...current, [row.id]: t('activity.action.failed', errorText(error)) }));
    } finally {
      pending.current.delete(key);
      setBusy(new Set(pending.current));
      onActionSettled?.();
    }
  };

  const notifications = useNotifications();
  const [tab, setTab] = useState<ActivityTab>(
    () => initialTab ?? (hasUnresolvedAlert(notifications) ? 'notifications' : 'progress'),
  );
  const [filters, setFilters] = useState<Record<ActivityTab, ActivityFilter>>(() => ({
    progress: readFilter('progress'),
    notifications: readFilter('notifications'),
  }));
  const changeFilter = (target: ActivityTab, filter: ActivityFilter) => {
    writeFilter(target, filter);
    setFilters(current => ({ ...current, [target]: filter }));
  };

  // Looking at the Notifications tab is reading it: the counter and the bell's dot clear.
  useEffect(() => {
    if (tab === 'notifications') markNotificationsSeen();
  }, [tab, notifications]);

  const outage = notifications.outage;
  const rows = sortRowsForPanel(overview?.rows ?? []);
  const counts = countByCategory(rows, categoryOfRow);
  const progressFilter = filters.progress;
  const visibleRows = progressFilter === 'all' ? rows : rows.filter(row => categoryOfRow(row) === progressFilter);
  const unavailable = overview?.unavailable_sources ?? [];
  const lastKnown = asOf ?? (overview ? new Date(overview.generated_at).getTime() : null);
  const unread = unreadCount(notifications);

  const renderRow = (row: ActivityRow) => (
    <ActivityRowItem
      key={row.id}
      row={row}
      now={now}
      stallAfterSecs={overview?.stall_after_secs ?? 0}
      busy={busy}
      stale={outage !== null}
      error={errors[row.id]}
      handlers={handlers}
      onOpenLink={onOpenLink}
      onPerform={perform}
    />
  );

  return createPortal(
    <div
      ref={rootRef}
      className="activity-panel"
      id="activity-panel"
      role="dialog"
      aria-modal="false"
      aria-label={t('activity.panel.title')}
      data-testid="activity-panel"
      tabIndex={-1}
    >
      <div className="activity-panel-header">
        <div className="activity-tabs" role="tablist" aria-label={t('activity.tabs.label')}>
          {(['progress', 'notifications'] as const).map(id => (
            <button
              key={id}
              type="button"
              role="tab"
              id={`activity-tab-${id}`}
              className="activity-tab"
              data-testid={`activity-tab-${id}`}
              aria-selected={tab === id}
              aria-controls={`activity-tabpanel-${id}`}
              onClick={() => setTab(id)}
            >
              {t(`activity.tab.${id}`)}
              {id === 'notifications' && unread > 0 && (
                <span className="activity-tab-counter" data-testid="activity-tab-unread">{unread}</span>
              )}
            </button>
          ))}
        </div>
        <button
          type="button"
          className="activity-panel-close"
          onClick={closeAndRestoreFocus}
          aria-label={t('common.close')}
        >
          <X size={12} />
        </button>
      </div>

      <div
        className="activity-panel-body"
        role="tabpanel"
        id={`activity-tabpanel-${tab}`}
        aria-labelledby={`activity-tab-${tab}`}
      >
        {tab === 'notifications' ? (
          <NotificationsTab
            onClose={onClose}
            notifications={notifications}
            now={now}
            filter={filters.notifications}
            onFilterChange={filter => changeFilter('notifications', filter)}
          />
        ) : (
          <div className="activity-tabpanel" data-stale={outage ? 'true' : undefined} data-testid="progress-tab">
            <ActivityFilterChips value={progressFilter} counts={counts} onChange={filter => changeFilter('progress', filter)} />
            {outage && lastKnown !== null && (
              <div className="activity-note activity-stale-note" role="status" data-testid="activity-stale">
                {t('activity.stale.asOf', formatTimeOfDay(lastKnown))}
              </div>
            )}
            <div className="activity-tabbody activity-progress-body">
              {visibleRows.length === 0 ? (
                <p className="activity-empty">
                  {overview ? t('activity.panel.empty') : t('activity.panel.loading')}
                </p>
              ) : progressFilter === 'all' ? (
                ACTIVITY_CATEGORIES.map(category => {
                  const section = visibleRows.filter(row => categoryOfRow(row) === category);
                  if (section.length === 0) return null;
                  const headingId = `activity-section-${category}`;
                  return (
                    <section key={category} className="activity-section" data-section={category} aria-labelledby={headingId}>
                      <h3 id={headingId} className="activity-section-title">
                        {t(`activity.filter.${category}`)}
                        <span className="activity-compartment-count">{section.length}</span>
                      </h3>
                      <ul className="activity-list">{section.map(renderRow)}</ul>
                    </section>
                  );
                })
              ) : (
                <ul className="activity-list">{visibleRows.map(renderRow)}</ul>
              )}
            </div>
          </div>
        )}
      </div>

      {tab === 'progress' && unavailable.length > 0 && (
        <div className="activity-note" role="status">
          {t('activity.panel.unavailable', unavailable.join(', '))}
        </div>
      )}
      {tab === 'progress' && failed && !outage && (
        <div className="activity-note" role="alert">{t('activity.panel.loadError')}</div>
      )}
    </div>,
    document.body,
  );
}

interface RowItemProps {
  row: ActivityRow;
  now: number;
  stallAfterSecs: number;
  busy: ReadonlySet<string>;
  /** The backend is unreachable: the row is the last known state and cannot be acted on. */
  stale?: boolean;
  error?: string;
  handlers: Readonly<Record<string, ActivityActionHandler>>;
  onOpenLink: (link: ActivityLink) => void;
  onPerform: (row: ActivityRow, action: ActivityAction) => void;
}

function ActivityRowItem({ row, now, busy, stale = false, error, handlers, onOpenLink, onPerform }: RowItemProps) {
  const { t } = useT();
  const started = new Date(row.started_at).getTime();
  const elapsed = formatDuration(now - started);
  const typeLabel = translateOr(t, `activity.type.${row.type}`, humanizeType(row.type));
  const stateLabel = translateOr(t, `activity.state.${row.state}`, humanizeType(row.state));
  const link = row.link ?? null;
  const silentFor = row.last_activity_at ? formatDuration(now - new Date(row.last_activity_at).getTime()) : null;

  const wake = row.state === 'paused'
    ? (row.wake_at
      ? t('activity.resumesAt', new Date(row.wake_at).toLocaleString())
      : t('activity.pausedParked'))
    : null;

  const body = (
    <>
      {row.state === 'paused'
        ? <Clock size={12} className="text-warning" aria-hidden="true" />
        : <Loader2 size={12} className={row.state === 'running' ? 'spin text-accent' : 'text-accent'} aria-hidden="true" />}
      <span className="activity-row-text">
        <span className="activity-row-name">{row.title}</span>
        <span className="activity-row-meta">
          {row.compartment !== 'running' && (
            <>
              <span className="activity-row-compartment" data-testid="activity-row-compartment">
                {t(`activity.compartment.${row.compartment}`)}
              </span>
              <span className="activity-row-sep" aria-hidden="true">·</span>
            </>
          )}
          <span>{typeLabel}</span>
          <span className="activity-row-sep" aria-hidden="true">·</span>
          <span data-testid="activity-row-state">{stateLabel}</span>
          {row.project_name && (
            <>
              <span className="activity-row-sep" aria-hidden="true">·</span>
              <span>{row.project_name}</span>
            </>
          )}
          {row.detail && (
            <>
              <span className="activity-row-sep" aria-hidden="true">·</span>
              <span>{row.detail}</span>
            </>
          )}
        </span>
      </span>
      <span
        className="activity-row-elapsed"
        data-testid="activity-row-elapsed"
        aria-label={t('activity.elapsedLabel', elapsed)}
      >
        {t('activity.elapsed', elapsed)}
      </span>
    </>
  );

  return (
    <li
      className="activity-row"
      data-testid="activity-row"
      data-row-type={row.type}
      data-state={row.state}
      data-stalled={row.stalled ? 'true' : undefined}
    >
      <div className="activity-row-main">
        {link ? (
          <button
            type="button"
            className="activity-row-link"
            aria-label={t('activity.open', row.title)}
            {...newTabClickProps(activityLinkPath(link))}
            onClick={() => onOpenLink(link)}
          >
            {body}
          </button>
        ) : (
          <div className="activity-row-link">{body}</div>
        )}
      </div>

      {(row.stalled || wake) && (
        <div className="activity-row-flags">
          {row.stalled && (
            <span className="activity-stalled" data-testid="activity-stalled" title={t('activity.stalledHint')}>
              <TriangleAlert size={11} aria-hidden="true" />
              {t('activity.stalled', silentFor ?? '')}
            </span>
          )}
          {wake && <span className="activity-wake" data-testid="activity-wake">{wake}</span>}
        </div>
      )}

      {row.actions.some(action => handlers[action.id]) && (
        <div className="activity-row-actions">
          {row.actions.filter(action => handlers[action.id]).map(action => {
            const working = busy.has(`${row.id}|${action.id}`);
            const label = translateOr(t, actionLabelKey(action), humanizeType(action.kind));
            return (
              <button
                key={action.id}
                type="button"
                className="activity-action-btn"
                data-kind={action.kind}
                data-testid={`activity-action-${action.id}`}
                disabled={working || stale}
                aria-label={t('activity.action.on', label, row.title)}
                onClick={() => onPerform(row, action)}
              >
                {working ? <Loader2 size={10} className="spin" aria-hidden="true" /> : ACTION_ICON[action.kind]}
                {working ? t('activity.action.busy') : label}
              </button>
            );
          })}
        </div>
      )}

      {error && <div className="activity-row-error" role="alert">{error}</div>}
    </li>
  );
}

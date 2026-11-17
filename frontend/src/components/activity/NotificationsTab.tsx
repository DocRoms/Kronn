import { useState } from 'react';
import { Check, CircleAlert, Copy, RefreshCw, TriangleAlert } from 'lucide-react';
import { useT } from '../../lib/I18nContext';
import { BACKEND_CHECK_NOW_EVENT } from '../../hooks/useBackendMonitor';
import { formatTimeOfDay, splitOutageDuration } from '../../lib/activity';
import {
  ACTIVITY_CATEGORIES,
  countByCategory,
  type ActivityFilter,
} from '../../lib/activityFilters';
import {
  acknowledgeServerError,
  clearNotificationHistory,
  dismissNotice,
  isUrgentNotice,
  serverErrorDetails,
  type HistoryEntry,
  type NoticeEntry,
  type NotificationCategory,
  type NotificationsSnapshot,
  type ServerErrorEntry,
} from '../../lib/notifications';
import { AppLink } from '../AppLink';
import { ActivityFilterChips } from './ActivityFilterChips';

export interface NotificationsTabProps {
  notifications: NotificationsSnapshot;
  now: number;
  filter: ActivityFilter;
  onFilterChange: (filter: ActivityFilter) => void;
  /** A notice's link was followed: the panel closes. */
  onClose?: () => void;
}

type Item =
  | { kind: 'notice'; key: string; category: NotificationCategory; entry: NoticeEntry }
  | { kind: 'error'; key: string; category: NotificationCategory; entry: ServerErrorEntry }
  | { kind: 'history'; key: string; category: NotificationCategory; entry: HistoryEntry };

/**
 * The bell's Notifications tab (KT-1684): the outage pinned in red whatever the
 * filter, then the server errors still pending (orange) and the session's
 * history, under the same category chips as the other tab.
 */
/** `10 s`, `1 min 12`, `2 h 05`: worded per locale, never as a clock. */
function outageLength(t: (key: string, ...args: (string | number)[]) => string, ms: number): string {
  const { unit, first, second } = splitOutageDuration(ms);
  return t(`notif.duration.${unit}`, first, second);
}

export function NotificationsTab({ notifications, now, filter, onFilterChange, onClose }: NotificationsTabProps) {
  const { t } = useT();
  const { outage, notices, errors, history } = notifications;

  const items: Item[] = [
    ...notices.map((entry): Item => ({ kind: 'notice', key: entry.id, category: entry.category, entry })),
    ...errors.map((entry): Item => ({ kind: 'error', key: entry.id, category: entry.category, entry })),
    ...history.map((entry): Item => ({ kind: 'history', key: entry.id, category: entry.category, entry })),
  ];
  const counts = countByCategory(items, item => item.category);
  // The pinned outage is a Kronn entry: it counts in All and Kronn, but is never filtered out.
  if (outage) {
    counts.all += 1;
    counts.kronn += 1;
  }
  const visible = filter === 'all' ? items : items.filter(item => item.category === filter);

  const renderItem = (item: Item) =>
    item.kind === 'notice'
      ? <NoticeRow key={item.key} entry={item.entry} onClose={onClose} />
      : item.kind === 'error'
        ? <ServerErrorRow key={item.key} entry={item.entry} />
        : <HistoryRow key={item.key} entry={item.entry} />;

  return (
    <div className="activity-tabpanel" data-testid="notifications-tab">
      {outage && (
        <section className="notif-pinned" aria-label={t('notif.outage.section')}>
          <div className="notif-outage" role="alert" data-testid="notif-outage">
            <CircleAlert size={14} aria-hidden="true" className="notif-outage-icon" />
            <div className="notif-body">
              <span className="notif-title" data-testid="notif-outage-title">
                {t('notif.outage.title', outageLength(t, now - outage.since))}
              </span>
              <span className="notif-meta">{t('notif.outage.since', formatTimeOfDay(outage.since))}</span>
            </div>
            <button
              type="button"
              className="notif-btn"
              data-testid="notif-outage-retry"
              onClick={() => window.dispatchEvent(new Event(BACKEND_CHECK_NOW_EVENT))}
            >
              <RefreshCw size={10} aria-hidden="true" />
              {t('notif.outage.retry')}
            </button>
          </div>
        </section>
      )}

      <ActivityFilterChips value={filter} counts={counts} onChange={onFilterChange} />

      <div className="activity-tabbody">
        {visible.length === 0 ? (
          // "Nothing to report" would contradict the pinned outage, which is itself an entry.
          items.length === 0
            ? !outage && <p className="activity-empty">{t('notif.empty')}</p>
            : <p className="activity-empty">{t('notif.empty.filtered')}</p>
        ) : filter === 'all' ? (
          ACTIVITY_CATEGORIES.map(category => {
            const section = visible.filter(item => item.category === category);
            if (section.length === 0) return null;
            return (
              <section key={category} className="activity-section" data-section={category} aria-labelledby={`notif-section-${category}`}>
                <h3 id={`notif-section-${category}`} className="activity-section-title">
                  {t(`activity.filter.${category}`)}
                  <span className="activity-compartment-count">{section.length}</span>
                </h3>
                <ul className="activity-list">{section.map(renderItem)}</ul>
              </section>
            );
          })
        ) : (
          <ul className="activity-list">{visible.map(renderItem)}</ul>
        )}
      </div>

      {history.length > 0 && (
        <div className="notif-footer">
          <button type="button" className="notif-btn" data-testid="notif-clear" onClick={clearNotificationHistory}>
            {t('notif.history.clear')}
          </button>
        </div>
      )}
    </div>
  );
}

/** A notice of the install: what is wrong, the one place that fixes it, and
 *  a way to put it away when it is only information. */
function NoticeRow({ entry, onClose }: { entry: NoticeEntry; onClose?: () => void }) {
  const { t } = useT();
  const [first = '', second = ''] = entry.args ?? [];
  let title: string | null = null;
  let body: string;
  let linkText: string | null = null;
  switch (entry.kind) {
    case 'token_refused':
      title = t('authFailure.banner.title');
      body = t('authFailure.banner.body', entry.label ?? '');
      linkText = t('authFailure.banner.fix', entry.label ?? '');
      break;
    case 'key_locked':
      body = first ? `${t('keyLocked.banner')} ${t('keyLocked.clearCredentials', first)}` : t('keyLocked.banner');
      linkText = t('keyLocked.restoreLink');
      break;
    case 'key_beside_data':
      // Plain words here; the technical detail is in Settings > Recovery.
      title = t('notif.protect.title');
      body = t('notif.protect.body');
      linkText = t('notif.protect.create');
      break;
    case 'credentials_unavailable':
      body = t('keyLocked.credentialsUnavailable', first);
      break;
    case 'update_available':
      body = t('app.updateAvailable', first, second);
      linkText = t('app.updateOpenRelease');
      break;
    default:
      body = first;
  }
  return (
    <li
      className="notif-row"
      data-testid="notif-notice"
      data-kind={entry.kind}
      data-category={entry.category}
      // The banners these replace were alerts: what stops work is still announced.
      role={isUrgentNotice(entry) ? 'alert' : undefined}
    >
      <TriangleAlert size={13} aria-hidden="true" className="notif-error-icon" />
      <div className="notif-body">
        {title && <span className="notif-title">{title}</span>}
        <span className={title ? 'notif-meta notif-message' : 'notif-title notif-message'}>{body}</span>
        {(entry.link || entry.dismiss) && (
          <span className="notif-actions">
            {entry.link && linkText && (entry.external ? (
              <a href={entry.link} target="_blank" rel="noreferrer" className="notif-btn" data-testid={`notif-notice-link-${entry.id}`}>
                {linkText}
              </a>
            ) : (
              <span onClick={onClose}>
                <AppLink to={entry.link} className="notif-btn" data-testid={`notif-notice-link-${entry.id}`}>
                  {linkText}
                </AppLink>
              </span>
            ))}
            {entry.dismiss && (
              <button type="button" className="notif-btn" data-testid={`notif-notice-dismiss-${entry.id}`} onClick={() => dismissNotice(entry.id)}>
                {entry.kind !== 'key_beside_data' && <Check size={10} aria-hidden="true" />}
                {t(entry.kind === 'key_beside_data' ? 'notif.protect.later' : 'notif.error.ack')}
              </button>
            )}
          </span>
        )}
      </div>
    </li>
  );
}

function ServerErrorRow({ entry }: { entry: ServerErrorEntry }) {
  const { t } = useT();
  const [copied, setCopied] = useState(false);

  const copy = () => {
    navigator.clipboard.writeText(serverErrorDetails(entry)).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1000);
    }).catch(() => {
      // The clipboard can be refused on an insecure context: nothing to say.
    });
  };

  return (
    <li className="notif-row" data-testid="notif-error" data-status={entry.status} data-category={entry.category}>
      <TriangleAlert size={13} aria-hidden="true" className="notif-error-icon" />
      <div className="notif-body">
        <span className="notif-title">
          {`HTTP ${entry.status} · ${entry.method} ${entry.route}`}
          {entry.count > 1 && <span className="notif-count" data-testid="notif-error-count">{` · ×${entry.count}`}</span>}
          {` · ${formatTimeOfDay(entry.lastAt)}`}
        </span>
        {entry.message && <span className="notif-meta notif-message">{entry.message}</span>}
        <span className="notif-actions">
          <button type="button" className="notif-btn" data-testid="notif-error-copy" onClick={copy}>
            {copied ? <Check size={10} aria-hidden="true" /> : <Copy size={10} aria-hidden="true" />}
            {copied ? t('notif.error.copied') : t('notif.error.copy')}
          </button>
          <button
            type="button"
            className="notif-btn"
            data-testid="notif-error-ack"
            onClick={() => acknowledgeServerError(entry.id)}
          >
            <Check size={10} aria-hidden="true" />
            {t('notif.error.ack')}
          </button>
        </span>
      </div>
    </li>
  );
}

function HistoryRow({ entry }: { entry: HistoryEntry }) {
  const { t } = useT();
  if (entry.kind === 'outage') {
    return (
      <li className="notif-row notif-row-resolved" data-testid="notif-history" data-kind="outage" data-category={entry.category}>
        <Check size={13} aria-hidden="true" className="notif-resolved-icon" />
        <div className="notif-body">
          <span className="notif-title">{t('notif.outage.resolved', outageLength(t, entry.resolvedAt - entry.since))}</span>
          <span className="notif-meta">{t('notif.outage.resolvedAt', formatTimeOfDay(entry.since), formatTimeOfDay(entry.resolvedAt))}</span>
        </div>
      </li>
    );
  }
  const { error } = entry;
  return (
    <li className="notif-row notif-row-resolved" data-testid="notif-history" data-kind="server_error" data-category={entry.category}>
      <Check size={13} aria-hidden="true" className="notif-resolved-icon" />
      <div className="notif-body">
        <span className="notif-title">
          {`HTTP ${error.status} · ${error.method} ${error.route}`}
          {error.count > 1 && ` · ×${error.count}`}
        </span>
        <span className="notif-meta">{t('notif.error.acknowledgedAt', formatTimeOfDay(entry.resolvedAt))}</span>
      </div>
    </li>
  );
}

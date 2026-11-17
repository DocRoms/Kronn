// The notification store behind the bell's second tab (KT-1684).
//
// The API client is the one place that feeds it (`lib/api.ts`): a request that
// fails the way a stopped backend fails raises a *suspicion*, the health check
// confirms it as THE outage, and every request that gets an answer again
// resolves it. Other 5xx answers are grouped by status + method + route. The
// panel, the bell and the toasts only read this store — no call site changes.
//
// Session memory only: nothing here is persisted.

import { useSyncExternalStore } from 'react';
import { redactSecrets } from './bug-report';

/** What an entry is about, for the panel's filter chips. */
export type NotificationCategory = 'discussions' | 'automations' | 'kronn';

/** The one red entry: the backend does not answer. */
export interface OutageEntry {
  since: number;
  /** The panel has been looked at since it started. */
  seen: boolean;
}

/** An orange entry: one status + method + route, with how many times it happened. */
export interface ServerErrorEntry {
  id: string;
  status: number;
  method: string;
  route: string;
  /** The server's own message, already redacted by the backend and scrubbed again here. */
  message: string;
  count: number;
  firstAt: number;
  lastAt: number;
  seen: boolean;
  category: NotificationCategory;
}

/** An entry that is over: an outage that ended, or an error that was acknowledged. */
export type HistoryEntry =
  | { id: string; kind: 'outage'; since: number; resolvedAt: number; category: 'kronn' }
  | { id: string; kind: 'server_error'; resolvedAt: number; error: ServerErrorEntry; category: NotificationCategory };

/** A standing condition of the install: it stays until its cause is fixed,
 *  so it has no acknowledge button. The tab words it from `kind`. */
export type NoticeKind =
  | 'key_locked'
  | 'key_beside_data'
  | 'credentials_unavailable'
  | 'config_set_aside'
  | 'token_refused'
  | 'update_available'
  | 'reset_error';

export interface NoticeEntry {
  id: string;
  kind: NoticeKind;
  /** The connection a refused token belongs to. */
  label?: string;
  /** What the wording of this kind takes: a server message, versions, names. */
  args?: readonly string[];
  /** Where the owner fixes it, or reads more. */
  link?: string;
  /** The link leaves Kronn: it opens in a new tab. */
  external?: boolean;
  /** The owner may put it away; called once when they do. */
  dismiss?: () => void;
  seen: boolean;
  category: NotificationCategory;
}

export type NoticeReport = Omit<NoticeEntry, 'seen'>;

export interface NotificationsSnapshot {
  outage: OutageEntry | null;
  notices: readonly NoticeEntry[];
  errors: readonly ServerErrorEntry[];
  history: readonly HistoryEntry[];
}

const MAX_ERRORS = 50;
const MAX_HISTORY = 50;
const MAX_MESSAGE_CHARS = 500;

const EMPTY: NotificationsSnapshot = { outage: null, notices: [], errors: [], history: [] };

let state: NotificationsSnapshot = EMPTY;
let suspectSince: number | null = null;
let sequence = 0;
const listeners = new Set<() => void>();

function commit(next: NotificationsSnapshot): void {
  state = next;
  for (const listener of [...listeners]) listener();
}

export function getNotifications(): NotificationsSnapshot {
  return state;
}

export function subscribeNotifications(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useNotifications(): NotificationsSnapshot {
  return useSyncExternalStore(subscribeNotifications, getNotifications, getNotifications);
}

/** A gateway answer: the backend behind it is not there. */
export function isGatewayStatus(status: number): boolean {
  return status === 502 || status === 503 || status === 504;
}

// ─── Outage ──────────────────────────────────────────────────────────────

/** A request failed like a stopped backend (network error, 502/503/504). Only a
 *  suspicion: one bad endpoint never raises the alert, the health check decides. */
export function noteSuspectedOutage(now = Date.now()): void {
  if (suspectSince === null) suspectSince = now;
}

/** The health check could not reach the backend: it is the outage, counted from
 *  the first failure seen. */
export function confirmOutage(now = Date.now()): void {
  if (state.outage) return;
  const since = suspectSince ?? now;
  commit({ ...state, outage: { since, seen: false } });
}

/** A request got an answer: the suspicion is over, and so is the outage. */
export function noteBackendAnswered(now = Date.now()): void {
  suspectSince = null;
  const outage = state.outage;
  if (!outage) return;
  const entry: HistoryEntry = {
    id: `outage-${outage.since}`,
    kind: 'outage',
    since: outage.since,
    resolvedAt: now,
    category: 'kronn',
  };
  commit({ ...state, outage: null, history: [entry, ...state.history].slice(0, MAX_HISTORY) });
}

// ─── Notices ─────────────────────────────────────────────────────────────

/** What one source (`key`, `token`) reports now: it replaces what that source
 *  said before. A notice already read stays read. */
export function setNotices(source: string, reports: readonly NoticeReport[]): void {
  const prefix = `${source}:`;
  const kept = state.notices.filter(notice => !notice.id.startsWith(prefix));
  const next = reports.map(report => {
    const id = `${prefix}${report.id}`;
    const seen = state.notices.find(notice => notice.id === id)?.seen ?? false;
    return { ...report, id, seen };
  });
  if (next.length === 0 && kept.length === state.notices.length) return;
  commit({ ...state, notices: [...kept, ...next] });
}

/** Its cause is gone: the notice leaves the list, and its source is not told. */
export function dropNotice(id: string): void {
  if (!state.notices.some(notice => notice.id === id)) return;
  commit({ ...state, notices: state.notices.filter(notice => notice.id !== id) });
}

/** The owner put a notice away: its source is told, and it leaves the list. */
export function dismissNotice(id: string): void {
  const notice = state.notices.find(candidate => candidate.id === id);
  if (!notice) return;
  commit({ ...state, notices: state.notices.filter(candidate => candidate !== notice) });
  notice.dismiss?.();
}

// ─── Server errors ───────────────────────────────────────────────────────

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** `/api/projects/3f2a…/files?x=1` → `/api/projects/:id/files`: the route, not the call. */
export function normalizeRoute(path: string): string {
  const withoutQuery = path.split('?')[0].split('#')[0];
  return withoutQuery
    .split('/')
    .map(segment => (UUID.test(segment) || /^\d+$/.test(segment) || /^[0-9a-f]{16,}$/i.test(segment) ? ':id' : segment))
    .join('/');
}

const AUTOMATION_ROUTES = /^\/api\/(workflows?|quick-prompts?|quick-apis?|quick-exec|automations?|batch)(\/|$)/;
const DISCUSSION_ROUTES = /^\/api\/(discussions?|disc|messages?)(\/|$)/;

export function categoryOfRoute(route: string): NotificationCategory {
  if (DISCUSSION_ROUTES.test(route)) return 'discussions';
  if (AUTOMATION_ROUTES.test(route)) return 'automations';
  return 'kronn';
}

export interface ServerErrorReport {
  status: number;
  method: string;
  /** The request path, with or without the `/api` prefix and a query. */
  path: string;
  message?: string;
}

/** A 5xx that is not a gateway answer: grouped by status + method + route. */
export function recordServerError(report: ServerErrorReport, now = Date.now()): void {
  const method = report.method.toUpperCase();
  const route = normalizeRoute(report.path.startsWith('/api') ? report.path : `/api${report.path}`);
  const id = `${report.status} ${method} ${route}`;
  const message = redactSecrets((report.message ?? '').trim()).slice(0, MAX_MESSAGE_CHARS);
  const existing = state.errors.find(entry => entry.id === id);
  if (existing) {
    const updated: ServerErrorEntry = {
      ...existing,
      count: existing.count + 1,
      lastAt: now,
      message: message || existing.message,
      seen: false,
    };
    commit({ ...state, errors: state.errors.map(entry => (entry === existing ? updated : entry)) });
    return;
  }
  const created: ServerErrorEntry = {
    id,
    status: report.status,
    method,
    route,
    message,
    count: 1,
    firstAt: now,
    lastAt: now,
    seen: false,
    category: categoryOfRoute(route),
  };
  commit({ ...state, errors: [created, ...state.errors].slice(0, MAX_ERRORS) });
}

/** The owner has read it: the error leaves the list for the session history. */
export function acknowledgeServerError(id: string, now = Date.now()): void {
  const error = state.errors.find(entry => entry.id === id);
  if (!error) return;
  const entry: HistoryEntry = {
    id: `ack-${id}-${++sequence}`,
    kind: 'server_error',
    resolvedAt: now,
    error,
    category: error.category,
  };
  commit({
    ...state,
    errors: state.errors.filter(candidate => candidate !== error),
    history: [entry, ...state.history].slice(0, MAX_HISTORY),
  });
}

export function acknowledgeAllServerErrors(now = Date.now()): void {
  for (const error of [...state.errors]) acknowledgeServerError(error.id, now);
}

/** "Clear all": the session history is emptied. Pending entries stay. */
export function clearNotificationHistory(): void {
  if (state.history.length === 0) return;
  commit({ ...state, history: [] });
}

/** The Notifications tab is on screen: what is pending counts as read. */
export function markNotificationsSeen(): void {
  const unseenError = state.errors.some(entry => !entry.seen) || state.notices.some(notice => !notice.seen);
  if (!unseenError && (!state.outage || state.outage.seen)) return;
  commit({
    ...state,
    notices: state.notices.map(notice => (notice.seen ? notice : { ...notice, seen: true })),
    outage: state.outage ? { ...state.outage, seen: true } : null,
    errors: state.errors.map(entry => (entry.seen ? entry : { ...entry, seen: true })),
  });
}

// ─── Reading ─────────────────────────────────────────────────────────────

/** An alert nobody dealt with yet: the outage, a server error still pending,
 *  or a standing notice not read yet (read, it no longer takes the panel over). */
export function hasUnresolvedAlert(snapshot: NotificationsSnapshot): boolean {
  return snapshot.outage !== null || snapshot.errors.length > 0 || snapshot.notices.some(notice => !notice.seen);
}

/** What the tab's counter and the bell's orange dot show. */
export function unreadCount(snapshot: NotificationsSnapshot): number {
  return (snapshot.outage && !snapshot.outage.seen ? 1 : 0)
    + snapshot.notices.filter(notice => !notice.seen).length
    + snapshot.errors.filter(entry => !entry.seen).length;
}

/** The text a copy button puts on the clipboard: status, route, and the server's message. */
export function serverErrorDetails(entry: ServerErrorEntry): string {
  const lines = [
    `HTTP ${entry.status}`,
    `${entry.method} ${entry.route}`,
    `occurrences: ${entry.count}`,
    `first: ${new Date(entry.firstAt).toISOString()}`,
    `last: ${new Date(entry.lastAt).toISOString()}`,
  ];
  if (entry.message) lines.push(`message: ${entry.message}`);
  return lines.join('\n');
}

/** Whether a toast's text is a backend failure the bell already carries: such
 *  toasts collapse into one short "see in the bell". 4xx are not: they stay
 *  next to the action. */
export function isServerFailureMessage(message: string): boolean {
  return /\(HTTP 5\d\d\)|^HTTP 5\d\d\b|Impossible de contacter le serveur|Failed to fetch|NetworkError/.test(message);
}

/** For tests. */
export function resetNotifications(): void {
  state = EMPTY;
  suspectSince = null;
  sequence = 0;
  for (const listener of [...listeners]) listener();
}

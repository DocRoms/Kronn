import { beforeEach, describe, expect, it } from 'vitest';
import {
  acknowledgeServerError,
  categoryOfRoute,
  clearNotificationHistory,
  confirmOutage,
  getNotifications,
  hasUnresolvedAlert,
  isServerFailureMessage,
  markNotificationsSeen,
  noteBackendAnswered,
  noteSuspectedOutage,
  normalizeRoute,
  recordServerError,
  setNotices,
  subscribeNotifications,
  resetNotifications,
  serverErrorDetails,
  unreadCount,
} from '../notifications';

beforeEach(() => { resetNotifications(); });

describe('the outage', () => {
  it('is one entry however many requests fail, counted from the first failure', () => {
    noteSuspectedOutage(1_000);
    noteSuspectedOutage(2_000);
    confirmOutage(5_000);
    confirmOutage(6_000);
    expect(getNotifications().outage).toEqual({ since: 1_000, seen: false });
  });

  it('is only a suspicion until the health check confirms it', () => {
    noteSuspectedOutage(1_000);
    expect(getNotifications().outage).toBeNull();
    noteBackendAnswered(1_500);
    // The suspicion is gone: a later confirmation counts from its own time.
    confirmOutage(9_000);
    expect(getNotifications().outage?.since).toBe(9_000);
  });

  it('resolves on the next answer, into the history with how long it lasted', () => {
    confirmOutage(10_000);
    noteBackendAnswered(82_000);
    const snapshot = getNotifications();
    expect(snapshot.outage).toBeNull();
    expect(snapshot.history).toEqual([
      { id: 'outage-10000', kind: 'outage', since: 10_000, resolvedAt: 82_000, category: 'kronn' },
    ]);
    expect(hasUnresolvedAlert(snapshot)).toBe(false);
  });
});

describe('server errors', () => {
  const failure = { status: 500, method: 'post', path: '/projects/3f2a9c1e-1111-4222-8333-444455556666/files?x=1', message: 'boom' };

  it('groups by status + method + route: ids and queries do not split a group', () => {
    recordServerError(failure, 1_000);
    recordServerError({ ...failure, path: '/projects/aaaaaaaa-1111-4222-8333-444455556666/files', message: 'boom again' }, 2_000);
    recordServerError({ ...failure, status: 503 }, 3_000);
    const { errors } = getNotifications();
    // 503 is a gateway answer in the API client; the store itself groups whatever it is given.
    expect(errors.map(e => [e.id, e.count])).toEqual([
      ['503 POST /api/projects/:id/files', 1],
      ['500 POST /api/projects/:id/files', 2],
    ]);
    const group = errors[1];
    expect(group).toMatchObject({ status: 500, method: 'POST', route: '/api/projects/:id/files', firstAt: 1_000, lastAt: 2_000, message: 'boom again' });
  });

  it('keeps different routes and methods apart', () => {
    recordServerError({ status: 500, method: 'GET', path: '/projects' });
    recordServerError({ status: 500, method: 'POST', path: '/projects' });
    recordServerError({ status: 500, method: 'GET', path: '/workflows' });
    expect(getNotifications().errors).toHaveLength(3);
  });

  it('scrubs a secret that slipped into the server message, and caps its length', () => {
    recordServerError({ status: 500, method: 'GET', path: '/x', message: `token sk-${'a'.repeat(30)} ${'z'.repeat(900)}` });
    const message = getNotifications().errors[0].message;
    expect(message).toContain('sk-***REDACTED***');
    expect(message).not.toContain('aaaaaaaaaaaaaaaaaaaa');
    expect(message.length).toBeLessThanOrEqual(500);
  });

  it('sorts a route into the category its chip filters', () => {
    expect(categoryOfRoute('/api/discussions/:id/messages')).toBe('discussions');
    expect(categoryOfRoute('/api/workflows/:id/runs')).toBe('automations');
    expect(categoryOfRoute('/api/quick-prompts')).toBe('automations');
    expect(categoryOfRoute('/api/projects/:id')).toBe('kronn');
  });

  it('is acknowledged into the history, and the history clears without touching what is pending', () => {
    recordServerError({ status: 500, method: 'GET', path: '/a' }, 1_000);
    recordServerError({ status: 500, method: 'GET', path: '/b' }, 1_000);
    acknowledgeServerError('500 GET /api/a', 2_000);
    expect(getNotifications().errors.map(e => e.route)).toEqual(['/api/b']);
    expect(getNotifications().history).toHaveLength(1);
    expect(getNotifications().history[0]).toMatchObject({ kind: 'server_error', resolvedAt: 2_000 });

    clearNotificationHistory();
    expect(getNotifications().history).toEqual([]);
    expect(getNotifications().errors).toHaveLength(1);
  });

  it('copies status, route, count, times and message', () => {
    recordServerError({ status: 500, method: 'POST', path: '/api/things', message: 'db is down' }, Date.parse('2026-11-11T14:02:00Z'));
    recordServerError({ status: 500, method: 'POST', path: '/api/things', message: 'db is down' }, Date.parse('2026-11-11T14:03:00Z'));
    const text = serverErrorDetails(getNotifications().errors[0]);
    expect(text).toContain('HTTP 500');
    expect(text).toContain('POST /api/things');
    expect(text).toContain('occurrences: 2');
    expect(text).toContain('message: db is down');
    expect(text).toContain('2026-11-11T14:03:00.000Z');
  });
});

describe('unread', () => {
  it('counts the outage and each pending group once, and a new occurrence is unread again', () => {
    confirmOutage(1_000);
    recordServerError({ status: 500, method: 'GET', path: '/a' });
    recordServerError({ status: 500, method: 'GET', path: '/a' });
    expect(unreadCount(getNotifications())).toBe(2);

    markNotificationsSeen();
    expect(unreadCount(getNotifications())).toBe(0);
    // Seen is not resolved: the alert is still pending.
    expect(hasUnresolvedAlert(getNotifications())).toBe(true);

    recordServerError({ status: 500, method: 'GET', path: '/a' });
    expect(unreadCount(getNotifications())).toBe(1);
  });
});

describe('what a toast may say', () => {
  it('recognises a server failure, and leaves 4xx next to the action', () => {
    expect(isServerFailureMessage('Server error (HTTP 502)')).toBe(true);
    expect(isServerFailureMessage('Server error (HTTP 500) — oops')).toBe(true);
    expect(isServerFailureMessage('HTTP 503')).toBe(true);
    expect(isServerFailureMessage('Impossible de contacter le serveur. Vérifiez votre connexion.')).toBe(true);
    expect(isServerFailureMessage('Server error (HTTP 422) — missing field')).toBe(false);
    expect(isServerFailureMessage('Project path does not exist')).toBe(false);
  });

  it('normalises routes', () => {
    expect(normalizeRoute('/api/discussions/12/messages?limit=5')).toBe('/api/discussions/:id/messages');
    expect(normalizeRoute('/api/projects')).toBe('/api/projects');
  });
});

describe('the notices of a source (KT-1686)', () => {
  beforeEach(() => resetNotifications());

  it('replace what that source said, keep what was read, and leave the other sources alone', () => {
    setNotices('token', [{ id: 'a', kind: 'token_refused', label: 'A', category: 'automations' }]);
    setNotices('key', [{ id: 'locked', kind: 'key_locked', category: 'kronn' }]);
    markNotificationsSeen();
    setNotices('token', [
      { id: 'a', kind: 'token_refused', label: 'A', category: 'automations' },
      { id: 'b', kind: 'token_refused', label: 'B', category: 'automations' },
    ]);
    expect(getNotifications().notices.map(n => [n.id, n.seen])).toEqual([['key:locked', true], ['token:a', true], ['token:b', false]]);
    setNotices('token', []);
    expect(getNotifications().notices.map(n => n.id)).toEqual(['key:locked']);
  });

  it('redraw nothing when a source says the same thing again', () => {
    const report = [{ id: 'a', kind: 'token_refused' as const, label: 'A', link: '/plugins/a', category: 'automations' as const }];
    setNotices('token', report);
    let calls = 0;
    const stop = subscribeNotifications(() => { calls += 1; });
    setNotices('token', [...report]);
    expect(calls).toBe(0);
    setNotices('token', [{ ...report[0], label: 'Renamed' }]);
    expect(calls).toBe(1);
    stop();
  });
});

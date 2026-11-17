// KT-1684 — the API client is the one place that feeds the notification store:
// no call site changes.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getNotifications, confirmOutage, resetNotifications } from '../notifications';

function respond(status: number, body: unknown, json = true) {
  (globalThis.fetch as ReturnType<typeof vi.fn>).mockResolvedValueOnce({
    status,
    ok: status >= 200 && status < 300,
    headers: { get: (name: string) => (name === 'content-type' ? (json ? 'application/json' : 'text/html') : null) },
    json: () => Promise.resolve(body),
    text: () => Promise.resolve(String(body)),
  });
}

beforeEach(async () => {
  vi.stubGlobal('fetch', vi.fn());
  const { setApiBase } = await import('../api');
  setApiBase('');
  resetNotifications();
});

afterEach(() => { vi.restoreAllMocks(); });

describe('what the API client reports', () => {
  it('groups repeated 5xx on one route into one entry with the server message', async () => {
    const { projects } = await import('../api');
    for (let i = 0; i < 3; i += 1) respond(500, { success: false, data: null, error: 'DB error: locked' });
    for (let i = 0; i < 3; i += 1) await expect(projects.scan()).rejects.toThrow(/Server error \(HTTP 500\)/);
    const { errors } = getNotifications();
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatchObject({ id: '500 POST /api/projects/scan', count: 3, message: 'DB error: locked' });
  });

  it('records a 5xx that is not JSON with its trimmed body', async () => {
    const { projects } = await import('../api');
    respond(500, 'internal failure', false);
    await expect(projects.scan()).rejects.toThrow(/HTTP 500/);
    expect(getNotifications().errors[0]).toMatchObject({ status: 500, message: 'internal failure' });
  });

  it('does not record a 4xx: it stays next to the action', async () => {
    const { projects } = await import('../api');
    respond(422, 'missing field', false);
    await expect(projects.scan()).rejects.toThrow();
    respond(200, { success: false, data: null, error: 'Invalid path' });
    await expect(projects.scan()).rejects.toThrow('Invalid path');
    expect(getNotifications().errors).toEqual([]);
  });

  it('turns a gateway answer into a suspicion, never an orange entry; the outage itself waits for the health check', async () => {
    const { projects } = await import('../api');
    respond(502, '<html>Bad Gateway</html>', false);
    await expect(projects.scan()).rejects.toThrow(/HTTP 502/);
    expect(getNotifications().errors).toEqual([]);
    expect(getNotifications().outage).toBeNull();
    confirmOutage();
    expect(getNotifications().outage).not.toBeNull();
  });

  it('resolves the outage on the next request that gets an answer', async () => {
    const { projects } = await import('../api');
    confirmOutage();
    respond(200, { success: true, data: [], error: null });
    await projects.scan();
    expect(getNotifications().outage).toBeNull();
    expect(getNotifications().history[0]).toMatchObject({ kind: 'outage' });
  });

  it('does not take an unreachable network for an answer', async () => {
    const { projects } = await import('../api');
    confirmOutage();
    (globalThis.fetch as ReturnType<typeof vi.fn>).mockRejectedValueOnce(new TypeError('Failed to fetch'));
    await expect(projects.scan()).rejects.toThrow('Failed to fetch');
    expect(getNotifications().outage).not.toBeNull();
  });
});

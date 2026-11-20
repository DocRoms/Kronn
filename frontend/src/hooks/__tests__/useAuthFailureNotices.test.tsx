/** KT-1524 / KT-1686 — connections whose token is refused are notices of the bell. */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, waitFor, act } from '@testing-library/react';

const { mcps } = vi.hoisted(() => ({ mcps: { overview: vi.fn() } }));
vi.mock('../../lib/api', () => ({ mcps }));

import { useAuthFailureNotices } from '../useAuthFailureNotices';
import { getNotifications, markNotificationsSeen, resetNotifications } from '../../lib/notifications';

function Host() {
  useAuthFailureNotices();
  return null;
}

const config = (id: string, label: string, refused: boolean) => ({
  id,
  label,
  auth_failure: refused
    ? { kind: 'refused', http_status: 401, message: 'Bad credentials', failed_at: '2026-11-10T08:00:00Z' }
    : undefined,
});

describe('useAuthFailureNotices', () => {
  beforeEach(() => { vi.clearAllMocks(); resetNotifications(); });
  afterEach(() => { cleanup(); vi.useRealTimers(); });

  it('reports each refused connection with the card that fixes it', async () => {
    mcps.overview.mockResolvedValue({
      configs: [config('cfg-1', 'GitHub Euronews', true), config('cfg-2', 'Notion', false), config('cfg-3', 'Adobe', true)],
    });
    render(<Host />);
    await waitFor(() => expect(getNotifications().notices).toHaveLength(2));
    expect(getNotifications().notices.map(n => [n.id, n.kind, n.label, n.link, n.category])).toEqual([
      ['token:cfg-1', 'token_refused', 'GitHub Euronews', '/plugins/cfg-1', 'automations'],
      ['token:cfg-3', 'token_refused', 'Adobe', '/plugins/cfg-3', 'automations'],
    ]);
  });

  it('reports nothing when every connection is fine or the overview cannot be read', async () => {
    mcps.overview.mockResolvedValueOnce({ configs: [config('cfg-2', 'Notion', false)] });
    render(<Host />);
    await waitFor(() => expect(mcps.overview).toHaveBeenCalled());
    await act(async () => {});
    expect(getNotifications().notices).toHaveLength(0);
    cleanup();
    mcps.overview.mockRejectedValueOnce(new Error('offline'));
    render(<Host />);
    await waitFor(() => expect(mcps.overview).toHaveBeenCalledTimes(2));
    await act(async () => {});
    expect(getNotifications().notices).toHaveLength(0);
  });

  it('drops a notice once the token is accepted again, and keeps a read one read', async () => {
    vi.useFakeTimers();
    mcps.overview.mockResolvedValue({ configs: [config('cfg-1', 'GitHub', true), config('cfg-3', 'Adobe', true)] });
    render(<Host />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(getNotifications().notices).toHaveLength(2);
    markNotificationsSeen();

    mcps.overview.mockResolvedValue({ configs: [config('cfg-1', 'GitHub', false), config('cfg-3', 'Adobe', true)] });
    await act(async () => { await vi.advanceTimersByTimeAsync(120_000); });
    expect(getNotifications().notices.map(n => [n.id, n.seen])).toEqual([['token:cfg-3', true]]);
  });
});

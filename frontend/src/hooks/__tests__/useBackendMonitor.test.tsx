/**
 * The `/api/health` poller (it was the BackendStatus pill until KT-1684): it
 * draws nothing now. It confirms the outage in the notification store, resolves
 * it on the way back up, and keeps the shared health state and the
 * "recovered" announcement the data hooks retry on.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/react';

const mocks = vi.hoisted(() => ({ fetchHealth: vi.fn() }));

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return { ...real, fetchHealth: mocks.fetchHealth };
});

import { BACKEND_CHECK_NOW_EVENT, useBackendMonitor } from '../useBackendMonitor';
import { getBackendHealth, onBackendRecovered, reportBackendSuspect, setBackendHealth } from '../../lib/backendReachability';
import { getNotifications, resetNotifications } from '../../lib/notifications';

function Monitor() {
  useBackendMonitor();
  return null;
}

describe('useBackendMonitor', () => {
  beforeEach(() => {
    mocks.fetchHealth.mockReset();
    setBackendHealth('unknown');
    resetNotifications();
  });
  afterEach(() => { vi.useRealTimers(); });

  it('stays silent while the backend answers /api/health', async () => {
    vi.useFakeTimers();
    mocks.fetchHealth.mockResolvedValue({ ok: true });
    const { container } = render(<Monitor />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(container).toBeEmptyDOMElement();
    expect(getNotifications().outage).toBeNull();
    expect(getBackendHealth()).toBe('up');
  });

  it('confirms the outage when /api/health fails, once however long it lasts, and resolves it when it answers', async () => {
    vi.useFakeTimers({ now: 100_000 });
    mocks.fetchHealth.mockRejectedValue(new Error('ECONNREFUSED'));
    render(<Monitor />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(getNotifications().outage?.since).toBe(100_000);
    expect(getBackendHealth()).toBe('down');

    // Polls every 2 s while down: still the one entry, still counted from the first failure.
    await act(async () => { await vi.advanceTimersByTimeAsync(6_000); });
    expect(mocks.fetchHealth.mock.calls.length).toBeGreaterThanOrEqual(3);
    expect(getNotifications().outage?.since).toBe(100_000);

    mocks.fetchHealth.mockResolvedValue({ ok: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    const snapshot = getNotifications();
    expect(snapshot.outage).toBeNull();
    expect(snapshot.history[0]).toMatchObject({ kind: 'outage', since: 100_000 });
    expect(getBackendHealth()).toBe('up');
  });

  it('checks at once when a request fails, instead of waiting for the 30 s poll', async () => {
    vi.useFakeTimers();
    mocks.fetchHealth.mockResolvedValueOnce({ ok: true });
    render(<Monitor />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(mocks.fetchHealth).toHaveBeenCalledTimes(1);

    mocks.fetchHealth.mockRejectedValue(new Error('ECONNREFUSED'));
    await act(async () => {
      reportBackendSuspect();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(mocks.fetchHealth).toHaveBeenCalledTimes(2);
    expect(getNotifications().outage).not.toBeNull();
  });

  it('checks at once for the Retry button', async () => {
    vi.useFakeTimers();
    mocks.fetchHealth.mockRejectedValue(new Error('ECONNREFUSED'));
    render(<Monitor />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    const before = mocks.fetchHealth.mock.calls.length;
    await act(async () => {
      window.dispatchEvent(new Event(BACKEND_CHECK_NOW_EVENT));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(mocks.fetchHealth.mock.calls.length).toBe(before + 1);
  });

  it('announces the recovery once, so failed loads can retry', async () => {
    vi.useFakeTimers();
    const recovered = vi.fn();
    const stop = onBackendRecovered(recovered);
    mocks.fetchHealth
      .mockRejectedValueOnce(new Error('ECONNREFUSED'))
      .mockResolvedValue({ ok: true });
    render(<Monitor />);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(recovered).not.toHaveBeenCalled();

    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(recovered).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(40_000); });
    expect(recovered).toHaveBeenCalledTimes(1);
    stop();
  });
});

/**
 * Persistent backend-health pill — should stay invisible while
 * `/api/health` answers, and surface a red "backend offline" badge
 * when it stops. The test pins the happy-path silence (no chrome
 * noise) and the unhealthy-path visibility, both critical for the
 * UX contract.
 */

import { afterEach, beforeEach, describe, it, expect, vi } from 'vitest';
import { act, render, waitFor, screen } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

const mocks = vi.hoisted(() => ({
  fetchHealth: vi.fn(),
  getUiLanguage: vi.fn().mockResolvedValue('fr'),
}));

vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<object>('../../lib/api');
  return {
    ...real,
    fetchHealth: mocks.fetchHealth,
    config: { getUiLanguage: mocks.getUiLanguage },
  };
});

import { BackendStatus } from '../BackendStatus';
import {
  getBackendHealth,
  onBackendRecovered,
  reportBackendSuspect,
  setBackendHealth,
} from '../../lib/backendReachability';

describe('BackendStatus', () => {
  beforeEach(() => {
    mocks.fetchHealth.mockReset();
    setBackendHealth('unknown');
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('renders nothing while the backend answers /api/health', async () => {
    mocks.fetchHealth.mockResolvedValue({ ok: true, version: '0.7.1' });
    const { container } = render(
      <I18nProvider><BackendStatus /></I18nProvider>
    );
    await waitFor(() => expect(mocks.fetchHealth).toHaveBeenCalled());
    // Pill stays hidden — no chrome noise on healthy state.
    expect(container.querySelector('.kronn-backend-status')).toBeNull();
  });

  it('renders the red pill when /api/health throws', async () => {
    mocks.fetchHealth.mockRejectedValue(new Error('ECONNREFUSED'));
    render(<I18nProvider><BackendStatus /></I18nProvider>);
    // After the rejection settles, the pill is visible with the
    // localised label. Use findByRole to wait for the async update.
    const status = await screen.findByRole('status');
    expect(status).toHaveClass('kronn-backend-status');
    // A restart is the usual cause: the pill says it reconnects.
    expect(status).toHaveTextContent(/reconnect|reconnexion|reconectando|重新连接/i);
  });

  it('clears the pill once the backend recovers', async () => {
    vi.useFakeTimers();
    // First call fails, then succeeds.
    mocks.fetchHealth
      .mockRejectedValueOnce(new Error('ECONNREFUSED'))
      .mockResolvedValue({ ok: true, version: '0.7.1' });
    const { container } = render(
      <I18nProvider><BackendStatus /></I18nProvider>
    );
    // Flush the immediately-started health request without relying on
    // Testing Library's timer-based polling (fake timers are active here).
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(screen.getByRole('status')).toBeVisible();
    expect(container.querySelector('.kronn-backend-status')).not.toBeNull();

    // Offline retries are intentionally fast: a restarted backend should
    // clear the warning without making the user wait for the healthy 30s poll.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    expect(mocks.fetchHealth).toHaveBeenCalledTimes(2);
    expect(container.querySelector('.kronn-backend-status')).toBeNull();
  });

  it('checks at once when a request fails, instead of waiting for the 30 s poll', async () => {
    // A restart shorter than the healthy poll interval left pages empty with
    // no explanation: a failed call must surface the pill right away.
    vi.useFakeTimers();
    mocks.fetchHealth.mockResolvedValueOnce({ ok: true, version: '0.7.1' });
    const { container } = render(<I18nProvider><BackendStatus /></I18nProvider>);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(getBackendHealth()).toBe('up');

    mocks.fetchHealth.mockRejectedValue(new Error('ECONNREFUSED'));
    await act(async () => {
      reportBackendSuspect();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(mocks.fetchHealth).toHaveBeenCalledTimes(2);
    expect(container.querySelector('.kronn-backend-status')).not.toBeNull();
    expect(getBackendHealth()).toBe('down');
  });

  it('announces the recovery once, so failed loads can retry', async () => {
    vi.useFakeTimers();
    const recovered = vi.fn();
    const stop = onBackendRecovered(recovered);
    mocks.fetchHealth
      .mockRejectedValueOnce(new Error('ECONNREFUSED'))
      .mockResolvedValue({ ok: true, version: '0.7.1' });
    render(<I18nProvider><BackendStatus /></I18nProvider>);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(recovered).not.toHaveBeenCalled();

    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); });
    expect(recovered).toHaveBeenCalledTimes(1);
    expect(getBackendHealth()).toBe('up');

    // Further healthy polls do not announce it again.
    await act(async () => { await vi.advanceTimersByTimeAsync(40_000); });
    expect(recovered).toHaveBeenCalledTimes(1);
    stop();
  });
});

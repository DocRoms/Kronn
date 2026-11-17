// KT-1684 — the bell's alert states and the panel that opens by itself.

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { readFileSync } from 'node:fs';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../../lib/I18nContext';
import { confirmOutage, noteBackendAnswered, recordServerError, resetNotifications, setNotices } from '../../../lib/notifications';
import { ActivityBell, OUTAGE_AUTO_OPEN_MS } from '../ActivityBell';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

import { activity as activityApi } from '../../../lib/api';

async function renderBell() {
  await act(async () => {
    render(<I18nProvider><ActivityBell onOpenLink={vi.fn()} /></I18nProvider>);
  });
  return screen.getByTestId('activity-bell');
}

const tick = (ms: number) => act(async () => { await vi.advanceTimersByTimeAsync(ms); });

beforeEach(() => {
  resetNotifications();
  vi.mocked(activityApi.overview).mockResolvedValue({
    rows: [], stall_after_secs: 900, generated_at: '2026-11-10T12:00:00Z', unavailable_sources: [],
  });
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.clearAllMocks(); });

describe('the bell while the backend is unreachable', () => {
  it('is red with a "!" and says so in its accessible name', async () => {
    const bell = await renderBell();
    expect(bell).not.toHaveAttribute('data-alert');
    act(() => { confirmOutage(); });
    expect(bell).toHaveAttribute('data-alert', 'outage');
    expect(screen.getByTestId('activity-bell-alert')).toHaveTextContent('!');
    expect(bell).toHaveAccessibleName('Notifications — service local injoignable');
    expect(screen.getByTestId('activity-bell-live')).toHaveTextContent('service local injoignable');

    act(() => { noteBackendAnswered(); });
    expect(bell).not.toHaveAttribute('data-alert');
    expect(screen.queryByTestId('activity-bell-alert')).toBeNull();
  });

  it('pulses gently, and stays a steady red when the reader asked for less motion', () => {
    const css = readFileSync('src/components/activity/ActivityBell.css', 'utf8');
    expect(css).toMatch(/\.activity-bell\[data-alert="outage"\]\s*\{[^}]*animation:\s*activity-bell-alert-pulse/);
    const reduced = css.slice(css.lastIndexOf('@media (prefers-reduced-motion: reduce)'));
    expect(reduced).toMatch(/\.activity-bell\[data-alert="outage"\]\s*\{\s*animation:\s*none;/);
    // The red itself is not an animation: it holds without it.
    expect(css).toMatch(/\.activity-bell\[data-alert="outage"\]\s*\{[^}]*border-color:\s*var\(--kr-error\)/);
  });
});

describe('the orange dot', () => {
  it('shows for unread notifications and goes once they are read', async () => {
    const bell = await renderBell();
    expect(screen.queryByTestId('activity-bell-dot')).toBeNull();
    act(() => { recordServerError({ status: 500, method: 'GET', path: '/projects' }); });
    expect(screen.getByTestId('activity-bell-dot')).toBeInTheDocument();
    expect(bell).toHaveAttribute('data-unread', 'true');
    expect(bell).toHaveAccessibleName('Activité — rien en cours — 1 non lue(s)');

    await act(async () => { fireEvent.click(bell); });
    // Pending errors open the panel on Notifications: reading it clears the dot.
    expect(screen.getByTestId('activity-tab-notifications')).toHaveAttribute('aria-selected', 'true');
    expect(screen.queryByTestId('activity-bell-dot')).toBeNull();
  });
});

describe('the panel that opens by itself', () => {
  beforeEach(() => { vi.useFakeTimers({ now: 1_000_000, toFake: ['Date', 'setInterval', 'clearInterval', 'setTimeout', 'clearTimeout'] }); });

  it('stays shut for a short outage', async () => {
    await renderBell();
    act(() => { confirmOutage(Date.now()); });
    await tick(OUTAGE_AUTO_OPEN_MS - 2_000);
    expect(screen.queryByTestId('activity-panel')).toBeNull();
    act(() => { noteBackendAnswered(); });
    await tick(5_000);
    expect(screen.queryByTestId('activity-panel')).toBeNull();
  });

  it('opens on Notifications when the outage lasts about 10 s, and closes itself once it is resolved', async () => {
    await renderBell();
    act(() => { confirmOutage(Date.now()); });
    await tick(OUTAGE_AUTO_OPEN_MS + 1_000);
    expect(screen.getByTestId('activity-panel')).toBeInTheDocument();
    expect(screen.getByTestId('activity-tab-notifications')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('notif-outage')).toBeInTheDocument();

    act(() => { noteBackendAnswered(); });
    expect(screen.queryByTestId('activity-panel')).toBeNull();
  });

  it('does not close a panel the owner took over, and does not reopen one they closed', async () => {
    const bell = await renderBell();
    act(() => { confirmOutage(Date.now()); });
    await tick(OUTAGE_AUTO_OPEN_MS + 1_000);
    expect(screen.getByTestId('activity-panel')).toBeInTheDocument();

    // The owner closes it: it stays closed for the rest of this outage.
    await act(async () => { fireEvent.click(bell); });
    expect(screen.queryByTestId('activity-panel')).toBeNull();
    await tick(5_000);
    expect(screen.queryByTestId('activity-panel')).toBeNull();

    // Opened by hand during the outage: it stays when the outage ends.
    await act(async () => { fireEvent.click(bell); });
    expect(screen.getByTestId('activity-panel')).toBeInTheDocument();
    act(() => { noteBackendAnswered(); });
    expect(screen.getByTestId('activity-panel')).toBeInTheDocument();
  });

  it('leaves a panel the owner already has open alone', async () => {
    const bell = await renderBell();
    await act(async () => { fireEvent.click(bell); });
    expect(screen.getByTestId('activity-tab-progress')).toHaveAttribute('aria-selected', 'true');
    act(() => { confirmOutage(Date.now()); });
    await tick(OUTAGE_AUTO_OPEN_MS + 1_000);
    expect(screen.getByTestId('activity-tab-progress')).toHaveAttribute('aria-selected', 'true');
    act(() => { noteBackendAnswered(); });
    expect(screen.getByTestId('activity-panel')).toBeInTheDocument();
  });
});

describe('the notices of the install (KT-1686)', () => {
  it('opens the panel on a notice that stops work or says an action failed, once', async () => {
    await renderBell();
    act(() => { setNotices('reset', [{ id: 'failed', kind: 'reset_error', args: ['disk full'], category: 'kronn', dismiss: () => {} }]); });
    expect(screen.getByTestId('notifications-tab')).toBeInTheDocument();
    expect(screen.getByRole('alert')).toHaveTextContent('disk full');

    // Closed by the owner: the same notice does not open it again.
    fireEvent.click(screen.getByTestId('activity-bell'));
    expect(screen.queryByTestId('notifications-tab')).toBeNull();
    act(() => { setNotices('token', [{ id: 'cfg-1', kind: 'token_refused', label: 'GitHub', link: '/plugins/cfg-1', category: 'automations' }]); });
    expect(screen.queryByTestId('notifications-tab')).toBeNull();
  });

  it('leaves the panel closed for advice, and says the unread count without moving focus', async () => {
    await renderBell();
    act(() => { setNotices('key', [{ id: 'beside-data', kind: 'key_beside_data', category: 'kronn' }]); });
    expect(screen.queryByTestId('activity-panel')).toBeNull();
    expect(screen.getByTestId('activity-bell-live')).toHaveTextContent('1 non lue(s)');
    expect(screen.getByTestId('activity-bell-dot')).toBeInTheDocument();
  });
});

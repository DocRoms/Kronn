// KT-1684 — the panel's two tabs, the Notifications tab and the category filters.

import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../../lib/I18nContext';
import { filterStorageKey } from '../../../lib/activityFilters';
import {
  confirmOutage,
  getNotifications,
  noteBackendAnswered,
  recordServerError,
  resetNotifications,
} from '../../../lib/notifications';
import type { ActivityOverview, ActivityRow } from '../../../types/generated';
import { ActivityPanel, type ActivityPanelProps } from '../ActivityPanel';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

const row = (overrides: Partial<ActivityRow> & Pick<ActivityRow, 'id' | 'type'>): ActivityRow => ({
  compartment: 'running', state: 'running', source: 'test', title: overrides.id,
  started_at: '2026-11-10T11:56:00Z', stalled: false, actions: [], ...overrides,
});

const ROWS: ActivityRow[] = [
  row({ id: 'turn-1', type: 'agent_turn', title: 'Fix the login' }),
  row({ id: 'turn-2', type: 'agent_turn', title: 'Write the docs' }),
  row({ id: 'run-1', type: 'workflow_run', title: 'Nightly review' }),
  row({ id: 'audit-1', type: 'audit', title: 'Audit Kronn' }),
];

const overview = (rows: ActivityRow[]): ActivityOverview => ({
  rows, stall_after_secs: 900, generated_at: '2026-11-10T12:00:00Z', unavailable_sources: [],
});

function renderPanel(props: Partial<ActivityPanelProps> = {}, rows: ActivityRow[] = ROWS) {
  render(
    <I18nProvider>
      <ActivityPanel overview={overview(rows)} onClose={vi.fn()} onOpenLink={vi.fn()} {...props} />
    </I18nProvider>,
  );
}

const fail = (path: string, status = 500, message = 'oops') => recordServerError({ status, method: 'GET', path, message });
const titles = () => screen.getAllByTestId('activity-row').map(el => within(el).getByText(/Fix|Write|Nightly|Audit/).textContent);

beforeEach(() => { localStorage.clear(); resetNotifications(); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

describe('the tabs and the default tab', () => {
  it('opens on In progress when nothing is unresolved, with the Notifications tab one click away', () => {
    renderPanel();
    expect(screen.getByTestId('activity-tab-progress')).toHaveAttribute('aria-selected', 'true');
    expect(screen.getByTestId('activity-tab-notifications')).toHaveAttribute('aria-selected', 'false');
    expect(screen.getByTestId('progress-tab')).toBeInTheDocument();
    expect(screen.queryByTestId('activity-tab-unread')).toBeNull();
    expect(screen.getByTestId('activity-tab-progress')).toHaveTextContent('En cours');
    expect(screen.getByTestId('activity-tab-notifications')).toHaveTextContent('Notifications');
  });

  it('opens on Notifications when an alert is unresolved: the outage, or a pending server error', () => {
    confirmOutage();
    renderPanel();
    expect(screen.getByTestId('activity-tab-notifications')).toHaveAttribute('aria-selected', 'true');
    cleanup();
    resetNotifications();

    fail('/projects');
    renderPanel();
    expect(screen.getByTestId('activity-tab-notifications')).toHaveAttribute('aria-selected', 'true');
  });

  it('stays on In progress when only resolved entries are left', () => {
    confirmOutage();
    noteBackendAnswered();
    renderPanel();
    expect(screen.getByTestId('activity-tab-progress')).toHaveAttribute('aria-selected', 'true');
  });

  it('honours the tab it is asked to open on', () => {
    renderPanel({ initialTab: 'notifications' });
    expect(screen.getByTestId('notifications-tab')).toBeInTheDocument();
  });

  it('counts what is unread on the Notifications tab, and reading the tab clears it', () => {
    fail('/projects');
    fail('/workflows');
    renderPanel({ initialTab: 'progress' });
    expect(screen.getByTestId('activity-tab-unread')).toHaveTextContent('2');

    fireEvent.click(screen.getByTestId('activity-tab-notifications'));
    expect(screen.queryByTestId('activity-tab-unread')).toBeNull();
    // Read is not resolved: both are still listed.
    expect(screen.getAllByTestId('notif-error')).toHaveLength(2);
  });

  it('makes a new occurrence unread again while the tab is on the other side', () => {
    fail('/projects');
    renderPanel({ initialTab: 'notifications' });
    fireEvent.click(screen.getByTestId('activity-tab-progress'));
    act(() => { fail('/projects'); });
    expect(screen.getByTestId('activity-tab-unread')).toHaveTextContent('1');
  });
});

describe('the Notifications tab', () => {
  it('shows the outage as one red entry with a clock and a Retry', () => {
    vi.useFakeTimers({ now: 100_000 });
    confirmOutage(100_000 - 42_000);
    renderPanel();
    const outage = screen.getByTestId('notif-outage');
    expect(outage).toHaveAttribute('role', 'alert');
    expect(within(outage).getByTestId('notif-outage-title')).toHaveTextContent('Service local injoignable depuis 0:42 — reconnexion…');
    expect(within(outage).getByTestId('notif-outage-retry')).toBeInTheDocument();
  });

  it('turns into "Rétabli après …" in the history when the backend answers again', () => {
    confirmOutage(10_000);
    renderPanel();
    expect(screen.getByTestId('notif-outage')).toBeInTheDocument();
    act(() => { noteBackendAnswered(82_000); });
    expect(screen.queryByTestId('notif-outage')).toBeNull();
    expect(screen.getByTestId('notif-history')).toHaveTextContent('Rétabli après 1m 12s');
  });

  it('groups other 5xx by status + method + route, with the count and the server message', () => {
    for (let i = 0; i < 3; i += 1) recordServerError({ status: 500, method: 'POST', path: '/projects/x', message: 'db locked' });
    renderPanel();
    const errors = screen.getAllByTestId('notif-error');
    expect(errors).toHaveLength(1);
    expect(errors[0]).toHaveTextContent('HTTP 500 · POST /api/projects/x · ×3');
    expect(errors[0]).toHaveTextContent('db locked');
  });

  it('acknowledges an error into the history, and "Tout effacer" empties the history', () => {
    fail('/projects');
    renderPanel();
    fireEvent.click(screen.getByTestId('notif-error-ack'));
    expect(screen.queryByTestId('notif-error')).toBeNull();
    expect(screen.getByTestId('notif-history')).toHaveTextContent('HTTP 500 · GET /api/projects');
    expect(getNotifications().errors).toEqual([]);

    fireEvent.click(screen.getByTestId('notif-clear'));
    expect(screen.queryByTestId('notif-history')).toBeNull();
    expect(screen.getByText('Rien à signaler.')).toBeInTheDocument();
  });

  it('copies the details: status, route, count and the redacted message', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    recordServerError({ status: 500, method: 'POST', path: '/things', message: `token sk-${'b'.repeat(30)}` });
    renderPanel();
    await act(async () => { fireEvent.click(screen.getByTestId('notif-error-copy')); });
    expect(writeText).toHaveBeenCalledTimes(1);
    const text = writeText.mock.calls[0][0] as string;
    expect(text).toContain('HTTP 500');
    expect(text).toContain('POST /api/things');
    expect(text).toContain('sk-***REDACTED***');
    expect(text).not.toContain('bbbbbbbbbb');
  });
});

describe('the category chips', () => {
  it('count what each category holds under both tabs, All being the total', () => {
    fail('/discussions/1/messages');
    fail('/workflows');
    fail('/projects');
    fail('/discussions/2/notes');
    renderPanel({ initialTab: 'notifications' });
    expect(screen.getByTestId('activity-chip-count-all')).toHaveTextContent('4');
    expect(screen.getByTestId('activity-chip-count-discussions')).toHaveTextContent('2');
    expect(screen.getByTestId('activity-chip-count-automations')).toHaveTextContent('1');
    expect(screen.getByTestId('activity-chip-count-kronn')).toHaveTextContent('1');
    fireEvent.click(screen.getByTestId('activity-tab-progress'));
    expect(screen.getByTestId('activity-chip-count-all')).toHaveTextContent('4');
    expect(screen.getByTestId('activity-chip-count-discussions')).toHaveTextContent('2');
    expect(screen.getByTestId('activity-chip-count-automations')).toHaveTextContent('1');
    expect(screen.getByTestId('activity-chip-count-kronn')).toHaveTextContent('1');
  });

  it('group "Tout" into three titled sections, and a single category into a flat list', () => {
    renderPanel();
    const sections = Array.from(document.querySelectorAll('[data-section]'));
    expect(sections.map(s => s.getAttribute('data-section'))).toEqual(['discussions', 'automations', 'kronn']);
    expect(screen.getByRole('heading', { name: /Discussions/ })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /Automatisations/ })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /Kronn/ })).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('activity-chip-discussions'));
    expect(screen.getByTestId('activity-chip-discussions')).toHaveAttribute('aria-pressed', 'true');
    expect(titles()).toEqual(['Fix the login', 'Write the docs']);
    expect(document.querySelector('[data-section]')).toBeNull();
  });

  it('group the Notifications list the same way', () => {
    fail('/discussions/1/messages');
    fail('/workflows');
    fail('/projects');
    renderPanel({ initialTab: 'notifications' });
    const sections = Array.from(document.querySelectorAll('[data-section]'));
    expect(sections.map(s => s.getAttribute('data-section'))).toEqual(['discussions', 'automations', 'kronn']);
    fireEvent.click(screen.getByTestId('activity-chip-automations'));
    const errors = screen.getAllByTestId('notif-error');
    expect(errors).toHaveLength(1);
    expect(errors[0]).toHaveAttribute('data-category', 'automations');
  });

  it('remember the last filter per tab, across openings', () => {
    renderPanel({ initialTab: 'progress' });
    fireEvent.click(screen.getByTestId('activity-chip-automations'));
    fireEvent.click(screen.getByTestId('activity-tab-notifications'));
    expect(screen.getByTestId('activity-chip-all')).toHaveAttribute('aria-pressed', 'true');
    fireEvent.click(screen.getByTestId('activity-chip-kronn'));
    expect(localStorage.getItem(filterStorageKey('progress'))).toBe('automations');
    expect(localStorage.getItem(filterStorageKey('notifications'))).toBe('kronn');

    cleanup();
    renderPanel({ initialTab: 'progress' });
    expect(screen.getByTestId('activity-chip-automations')).toHaveAttribute('aria-pressed', 'true');
    expect(titles()).toEqual(['Nightly review']);
    fireEvent.click(screen.getByTestId('activity-tab-notifications'));
    expect(screen.getByTestId('activity-chip-kronn')).toHaveAttribute('aria-pressed', 'true');
  });

  it('keep the red alert pinned whatever the filter, and out of the counts', () => {
    confirmOutage();
    fail('/discussions/1/messages');
    renderPanel({ initialTab: 'notifications' });
    for (const chip of ['discussions', 'automations', 'kronn', 'all']) {
      fireEvent.click(screen.getByTestId(`activity-chip-${chip}`));
      expect(screen.getByTestId('notif-outage')).toBeInTheDocument();
    }
    expect(screen.getByTestId('activity-chip-count-all')).toHaveTextContent('1');
    fireEvent.click(screen.getByTestId('activity-chip-automations'));
    expect(screen.queryByTestId('notif-error')).toBeNull();
    expect(screen.getByText('Rien dans cette catégorie.')).toBeInTheDocument();
  });
});

describe('during an outage', () => {
  it('shows the last known state greyed, "as of hh:mm", and no live action', () => {
    confirmOutage();
    const withAction = [row({
      id: 'turn-1', type: 'agent_turn', title: 'Fix the login',
      actions: [{ id: 'stop_dispatch', kind: 'stop', args: { discussion_id: 'd', dispatch_id: 'j' } }],
    })];
    renderPanel({ initialTab: 'progress', asOf: Date.parse('2026-11-10T11:58:00Z') }, withAction);
    expect(screen.getByTestId('progress-tab')).toHaveAttribute('data-stale', 'true');
    expect(screen.getByTestId('activity-stale')).toHaveTextContent(/Dernier état connu, à \d{2}:\d{2}/);
    expect(screen.getByTestId('activity-action-stop_dispatch')).toBeDisabled();
    // It is the outage that explains a stale view, not the generic "may be out of date".
    cleanup();
    renderPanel({ initialTab: 'progress', failed: true });
    expect(screen.queryByTestId('progress-tab')?.getAttribute('data-stale')).toBeTruthy();
  });

  it('is not greyed once the backend is back', () => {
    renderPanel({ initialTab: 'progress' });
    expect(screen.getByTestId('progress-tab')).not.toHaveAttribute('data-stale');
    expect(screen.queryByTestId('activity-stale')).toBeNull();
  });
});

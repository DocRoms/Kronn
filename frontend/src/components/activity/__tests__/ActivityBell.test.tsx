import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../../lib/I18nContext';
import type { ActivityOverview, ActivityRow } from '../../../types/generated';
import { ActivityBell } from '../ActivityBell';

vi.mock('../../../lib/api', async () => {
  const { buildApiMock } = await import('../../../test/apiMock');
  return buildApiMock();
});

import { activity as activityApi } from '../../../lib/api';

const row = (overrides: Partial<ActivityRow> & Pick<ActivityRow, 'id' | 'type'>): ActivityRow => ({
  compartment: 'running', state: 'running', source: 'test', title: overrides.id,
  started_at: '2026-11-10T11:56:00Z', stalled: false, actions: [], ...overrides,
});

const overview = (rows: ActivityRow[]): ActivityOverview => ({
  rows, stall_after_secs: 900, generated_at: '2026-11-10T12:00:00Z', unavailable_sources: [],
});

function serve(rows: ActivityRow[]) {
  vi.mocked(activityApi.overview).mockResolvedValue(overview(rows));
}

async function renderBell(props: Partial<React.ComponentProps<typeof ActivityBell>> = {}) {
  await act(async () => {
    render(<I18nProvider><ActivityBell onOpenLink={vi.fn()} {...props} /></I18nProvider>);
  });
  return screen.getByTestId('activity-bell');
}

beforeEach(() => { serve([]); });
afterEach(() => { cleanup(); vi.useRealTimers(); vi.clearAllMocks(); });

describe('ActivityBell', () => {
  it('is always in the header, even with nothing running, and it stands still', async () => {
    const bell = await renderBell();
    expect(bell).toBeInTheDocument();
    expect(bell).toHaveAttribute('data-running', 'false');
    expect(bell).toHaveAccessibleName('Activité — rien en cours');
    expect(screen.queryByTestId('activity-bell-badge')).toBeNull();
  });

  it('animates while something runs and says how many in its accessible name', async () => {
    serve([
      row({ id: 'a', type: 'agent_turn' }),
      row({ id: 'b', type: 'workflow_run', state: 'queued' }),
    ]);
    const bell = await renderBell();
    expect(bell).toHaveAttribute('data-running', 'true');
    expect(bell).toHaveAccessibleName('Activité — 2 en cours');
  });

  it('does not animate for a run that is only paused on a provider quota', async () => {
    serve([row({ id: 'p', type: 'workflow_run', state: 'paused', wake_at: '2026-11-11T02:02:00Z' })]);
    const bell = await renderBell();
    expect(bell).toHaveAttribute('data-running', 'false');
    expect(bell).toHaveAccessibleName('Activité — 1 en cours');
  });

  it('stops animating once what ran has finished, on the next poll', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    serve([row({ id: 'a', type: 'agent_turn' })]);
    const bell = await renderBell();
    expect(bell).toHaveAttribute('data-running', 'true');

    serve([]);
    await act(async () => { vi.advanceTimersByTime(3_000); });
    expect(bell).toHaveAttribute('data-running', 'false');
  });

  it('polls quickly while something runs and slowly when idle', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    await renderBell();
    expect(activityApi.overview).toHaveBeenCalledTimes(1);
    await act(async () => { vi.advanceTimersByTime(14_000); });
    expect(activityApi.overview).toHaveBeenCalledTimes(1);
    await act(async () => { vi.advanceTimersByTime(1_000); });
    expect(activityApi.overview).toHaveBeenCalledTimes(2);
  });

  it('carries a counter for what waits for the owner', async () => {
    serve([
      row({ id: 'ask1', type: 'agent_turn', compartment: 'needs_decision' }),
      row({ id: 'ask2', type: 'agent_turn', compartment: 'needs_decision' }),
      row({ id: 'run', type: 'agent_turn' }),
    ]);
    const bell = await renderBell();
    expect(screen.getByTestId('activity-bell-badge')).toHaveTextContent('2');
    expect(bell).toHaveAccessibleName('Activité — 2 vous attendent, 1 en cours');
  });

  it('opens the panel on click and closes it on a second click, keeping its disclosure state honest', async () => {
    serve([row({ id: 'a', type: 'agent_turn', title: 'Fix the login' })]);
    const bell = await renderBell();
    expect(bell).toHaveAttribute('aria-haspopup', 'dialog');
    expect(bell).toHaveAttribute('aria-expanded', 'false');
    expect(bell).not.toHaveAttribute('aria-controls');

    await act(async () => { fireEvent.click(bell); });
    expect(screen.getByRole('dialog', { name: 'Activité' })).toHaveTextContent('Fix the login');
    expect(bell).toHaveAttribute('aria-expanded', 'true');
    expect(bell).toHaveAttribute('aria-controls', 'activity-panel');

    // A pointerdown on the trigger is not an outside click: the click that
    // follows closes the panel instead of reopening it.
    await act(async () => {
      fireEvent.pointerDown(bell);
      fireEvent.click(bell);
    });
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(bell).toHaveAttribute('aria-expanded', 'false');
  });

  it('shows the current state when opened, not the last poll', async () => {
    const bell = await renderBell();
    expect(activityApi.overview).toHaveBeenCalledTimes(1);
    serve([row({ id: 'new', type: 'agent_turn', title: 'Started a second ago' })]);
    await act(async () => { fireEvent.click(bell); });
    expect(activityApi.overview).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('dialog')).toHaveTextContent('Started a second ago');
  });

  it('closes the panel and hands the link over when a row is opened', async () => {
    serve([row({ id: 'a', type: 'agent_turn', title: 'Fix the login', link: { target: 'discussion', id: 'd1' } })]);
    const onOpenLink = vi.fn();
    const bell = await renderBell({ onOpenLink });
    await act(async () => { fireEvent.click(bell); });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Ouvrir Fix the login' })); });
    expect(onOpenLink).toHaveBeenCalledWith({ target: 'discussion', id: 'd1' });
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('refreshes itself and its host after an action settles', async () => {
    serve([row({
      id: 'a', type: 'agent_turn', title: 'Fix the login',
      actions: [{ id: 'stop_dispatch', kind: 'stop', args: { discussion_id: 'd1', dispatch_id: 'j1' } }],
    })]);
    const stop = vi.fn().mockResolvedValue(undefined);
    const onActionSettled = vi.fn();
    const bell = await renderBell({ handlers: { stop_dispatch: stop }, onActionSettled });
    await act(async () => { fireEvent.click(bell); });
    const before = vi.mocked(activityApi.overview).mock.calls.length;

    await act(async () => { fireEvent.click(screen.getByRole('button', { name: /Arrêter : Fix the login/ })); });
    expect(stop).toHaveBeenCalledWith({ discussion_id: 'd1', dispatch_id: 'j1' });
    expect(onActionSettled).toHaveBeenCalledTimes(1);
    expect(vi.mocked(activityApi.overview).mock.calls.length).toBeGreaterThan(before);
  });

  it('keeps the last good view when a poll fails, and says so in the panel', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] });
    serve([row({ id: 'a', type: 'agent_turn', title: 'Fix the login' })]);
    const bell = await renderBell();
    vi.mocked(activityApi.overview).mockRejectedValue(new Error('offline'));
    await act(async () => { vi.advanceTimersByTime(3_000); });
    expect(bell).toHaveAttribute('data-running', 'true');
    await act(async () => { fireEvent.click(bell); });
    expect(screen.getByRole('dialog')).toHaveTextContent('Fix the login');
    expect(screen.getByRole('alert')).toHaveTextContent('périmé');
  });
});

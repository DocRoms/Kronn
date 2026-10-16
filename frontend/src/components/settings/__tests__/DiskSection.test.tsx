// DiskSection (KT-1318) — free space, Kronn's usage by kind and task, and a purge
// that takes orphans by default and a named entry only after a confirmation.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';

const { diskApi, configApi } = vi.hoisted(() => ({
  diskApi: { report: vi.fn(), purgeOrphans: vi.fn(), purgeEntries: vi.fn() },
  configApi: { setServerConfig: vi.fn() },
}));

vi.mock('../../../lib/api', () => ({ diskApi, config: configApi }));

import { DiskSection } from '../DiskSection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;
const GB = 1024 ** 3;

function entry(overrides: Record<string, unknown> = {}) {
  return {
    project_id: 'p1',
    kind: 'lane_build_dir',
    path: '/repo/target/wt-kt-1-aaaa',
    name: 'wt-kt-1-aaaa',
    bytes: 12 * GB,
    bytes_are_partial: false,
    idle_seconds: 8 * 3600,
    owner: { task_ref: 'KT-1', execution_id: 'exec-1', status: 'done' },
    state: 'orphan',
    reason: null,
    purgeable: true,
    ...overrides,
  };
}

function report(overrides: Record<string, unknown> = {}) {
  return {
    available_bytes: 20 * GB,
    under_pressure: false,
    policy: { launch_min_free_gib: 30, sweep_idle_hours: 6, warning_gib: 20, critical_gib: 5 },
    volumes: [],
    totals: [
      { kind: 'worktree', entries: 1, bytes: GB, bytes_are_partial: false, orphan_bytes: 0 },
      { kind: 'lane_build_dir', entries: 3, bytes: 40 * GB, bytes_are_partial: true, orphan_bytes: 12 * GB },
      { kind: 'cache', entries: 0, bytes: 0, bytes_are_partial: false, orphan_bytes: 0 },
    ],
    orphan_bytes: 12 * GB,
    orphan_count: 1,
    entries: [
      entry(),
      entry({
        path: '/repo/target/wt-kt-2-bbbb',
        name: 'wt-kt-2-bbbb',
        owner: { task_ref: 'KT-2', execution_id: 'exec-2', status: 'working' },
        state: 'active',
        reason: 'execution exec-2 is working',
        purgeable: false,
        idle_seconds: 60,
      }),
      entry({
        path: '/repo/target/wt-0151-integration',
        name: 'wt-0151-integration',
        owner: null,
        state: 'foreign',
        reason: 'no marker',
        purgeable: false,
      }),
    ],
    ...overrides,
  };
}

async function measured(next = report()) {
  diskApi.report.mockResolvedValue(next);
  render(<DiskSection toast={vi.fn()} t={t} />);
  fireEvent.click(screen.getByTestId('disk-measure'));
  await screen.findByTestId('disk-entries');
}

beforeEach(() => {
  vi.clearAllMocks();
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe('DiskSection', () => {
  it('measures only on demand, never on mount', () => {
    render(<DiskSection toast={vi.fn()} t={t} />);
    expect(diskApi.report).not.toHaveBeenCalled();
    expect(screen.getByTestId('disk-measure')).toBeTruthy();
  });

  it('shows free space, totals by kind and each entry with its task, state and age', async () => {
    await measured();
    expect(screen.getByTestId('disk-free').textContent).toContain('20 Go');
    const totals = screen.getByTestId('disk-totals');
    expect(within(totals).getByText('≥ 40 Go')).toBeTruthy();
    const rows = screen.getAllByTestId('disk-entry');
    expect(rows).toHaveLength(3);
    expect(rows[0].textContent).toContain('KT-1 · done');
    expect(rows[0].textContent).toContain('12 Go');
    expect(rows[0].textContent).toContain('settings.disk.hours(8)');
    expect(rows[0].getAttribute('data-state')).toBe('orphan');
    expect(rows[1].textContent).toContain('KT-2 · working');
    expect(rows[2].textContent).toContain('—');
  });

  it('offers a delete only on entries a confirmed delete may take', async () => {
    await measured();
    const rows = screen.getAllByTestId('disk-entry');
    expect(within(rows[0]).queryByTestId('disk-delete-entry')).toBeTruthy();
    expect(within(rows[1]).queryByTestId('disk-delete-entry')).toBeNull();
    expect(within(rows[2]).queryByTestId('disk-delete-entry')).toBeNull();
  });

  it('purges orphans only after a confirmation, then reloads', async () => {
    await measured();
    diskApi.purgeOrphans.mockResolvedValue({
      mode: 'orphans',
      items: [{ path: '/repo/target/wt-kt-1-aaaa', purged: true, bytes: 12 * GB, bytes_are_partial: false, reason: null }],
      freed_bytes: 12 * GB,
      kept: 2,
      available_bytes_after: 32 * GB,
    });
    const confirm = vi.fn().mockReturnValueOnce(false).mockReturnValueOnce(true);
    window.confirm = confirm as never;

    fireEvent.click(screen.getByTestId('disk-purge-orphans'));
    expect(diskApi.purgeOrphans).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('disk-purge-orphans'));
    await waitFor(() => expect(diskApi.purgeOrphans).toHaveBeenCalledTimes(1));
    expect(confirm).toHaveBeenCalledTimes(2);
    expect(diskApi.purgeEntries).not.toHaveBeenCalled();
    await waitFor(() => expect(diskApi.report).toHaveBeenCalledTimes(2));
  });

  it('disables the orphan purge when there is no orphan', async () => {
    await measured(report({ orphan_count: 0, orphan_bytes: 0 }));
    expect((screen.getByTestId('disk-purge-orphans') as HTMLButtonElement).disabled).toBe(true);
  });

  it('deletes one named entry only after a confirmation, naming it', async () => {
    await measured();
    diskApi.purgeEntries.mockResolvedValue({
      mode: 'entries',
      items: [{ path: '/repo/target/wt-kt-1-aaaa', purged: true, bytes: 12 * GB, bytes_are_partial: false, reason: null }],
      freed_bytes: 12 * GB,
      kept: 0,
      available_bytes_after: null,
    });
    const confirm = vi.fn().mockReturnValueOnce(false).mockReturnValueOnce(true);
    window.confirm = confirm as never;
    const button = within(screen.getAllByTestId('disk-entry')[0]).getByTestId('disk-delete-entry');

    fireEvent.click(button);
    expect(diskApi.purgeEntries).not.toHaveBeenCalled();
    fireEvent.click(button);

    await waitFor(() =>
      expect(diskApi.purgeEntries).toHaveBeenCalledWith(['/repo/target/wt-kt-1-aaaa'], 'p1'),
    );
    expect(String(confirm.mock.calls[1][0])).toContain('/repo/target/wt-kt-1-aaaa');
  });

  it('says so when the pressure line is crossed', async () => {
    await measured(report({ under_pressure: true }));
    expect(screen.getByTestId('disk-pressure').textContent).toContain('settings.disk.pressure(30)');
  });

  it('saves the two thresholds as whole numbers and refuses anything else', async () => {
    await measured();
    configApi.setServerConfig.mockResolvedValue(undefined);
    const [launch, idle] = within(screen.getByTestId('disk-policy')).getAllByRole('spinbutton');
    fireEvent.change(launch, { target: { value: '50' } });
    fireEvent.change(idle, { target: { value: '2' } });
    fireEvent.click(within(screen.getByTestId('disk-policy')).getByText('settings.disk.savePolicy'));
    await waitFor(() =>
      expect(configApi.setServerConfig).toHaveBeenCalledWith({
        disk_launch_min_free_gib: 50,
        disk_sweep_idle_hours: 2,
      }),
    );

    configApi.setServerConfig.mockClear();
    fireEvent.change(launch, { target: { value: '1.5' } });
    fireEvent.click(within(screen.getByTestId('disk-policy')).getByText('settings.disk.savePolicy'));
    expect(configApi.setServerConfig).not.toHaveBeenCalled();
  });

  it('surfaces a failing report instead of crashing', async () => {
    diskApi.report.mockRejectedValue(new Error('backend down'));
    render(<DiskSection toast={vi.fn()} t={t} />);
    fireEvent.click(screen.getByTestId('disk-measure'));
    expect((await screen.findByRole('alert')).textContent).toContain('backend down');
  });
});

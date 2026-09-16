// DbUsageChart — the answer to "what should I purge".
//
// The measurement it renders exists because row counts mislead: the instance
// that prompted it held 34 678 messages in 41 MB and 2 554 workflow runs in
// 2 620 MB. A chart that ranked by count would have pointed at the messages.

import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';

const { configApi } = vi.hoisted(() => ({
  configApi: { dbUsage: vi.fn() },
}));

vi.mock('../../../lib/api', () => ({ config: configApi }));
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string) => key, locale: 'en' }),
}));

import { DbUsageChart } from '../DbUsageChart';

const MB = 1024 * 1024;

function usage(overrides: Record<string, unknown> = {}) {
  return {
    file_bytes: 7_200 * MB,
    wal_bytes: 141 * MB,
    free_bytes: 0,
    tables: [
      { name: 'workflow_runs', bytes: 4_540 * MB, index_bytes: 2 * MB, rows: 7_122 },
      { name: 'shared_runs', bytes: 2_620 * MB, index_bytes: 1 * MB, rows: 2_554 },
      { name: 'messages', bytes: 41 * MB, index_bytes: 4 * MB, rows: 34_678 },
    ],
    ...overrides,
  };
}

describe('DbUsageChart', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    configApi.dbUsage.mockResolvedValue(usage());
  });

  it('does not measure until asked', async () => {
    // dbstat walks the b-trees — about a second on a 7 GB database. Opening
    // Settings must not cost that, so the measurement is an explicit action.
    render(<DbUsageChart />);
    expect(configApi.dbUsage).not.toHaveBeenCalled();
    expect(screen.getByText('config.dbUsage.measure')).toBeInTheDocument();
  });

  it('ranks by weight, not by row count', async () => {
    render(<DbUsageChart />);
    fireEvent.click(screen.getByRole('button', { name: /config.dbUsage.measure/ }));
    await waitFor(() => expect(screen.getByText('workflow_runs')).toBeInTheDocument());

    const rows = screen.getAllByRole('row');
    const order = rows.map(row => within(row).getAllByRole('cell')[0].textContent);
    expect(order).toEqual(['workflow_runs', 'shared_runs', 'messages']);

    // The table with the most rows by a factor of five is last, which is the
    // whole point: it is also the lightest.
    const messages = rows[2];
    expect(within(messages).getByText('41.0 Mo')).toBeInTheDocument();
  });

  it('folds the tail rather than dropping it, so the parts still add up', async () => {
    const tail = Array.from({ length: 12 }, (_, i) => ({
      name: `small_${i}`,
      bytes: 10 * MB,
      index_bytes: 0,
      rows: 100,
    }));
    configApi.dbUsage.mockResolvedValue(usage({ tables: [...usage().tables, ...tail] }));

    render(<DbUsageChart />);
    fireEvent.click(screen.getByRole('button', { name: /config.dbUsage.measure/ }));
    await waitFor(() => expect(screen.getByText('workflow_runs')).toBeInTheDocument());

    // 3 charted + 12 small = 15 tables, capped at 8 charted plus one fold.
    expect(screen.getAllByRole('row')).toHaveLength(9);
    expect(screen.getByText('config.dbUsage.otherTables')).toBeInTheDocument();

    const percentages = screen
      .getAllByRole('row')
      .map(row => Number(within(row).getAllByRole('cell')[2].textContent!.replace(' %', '')));
    const total = percentages.reduce((sum, value) => sum + value, 0);
    expect(total).toBeGreaterThan(99);
    expect(total).toBeLessThan(101);
  });

  it('reports freed pages separately, so a purge that frees no disk is legible', async () => {
    // Deleting rows returns pages to the freelist, not to the filesystem.
    // Without this line an operator purges, sees the same file size, and
    // concludes nothing happened.
    configApi.dbUsage.mockResolvedValue(usage({ free_bytes: 900 * MB }));

    render(<DbUsageChart />);
    fireEvent.click(screen.getByRole('button', { name: /config.dbUsage.measure/ }));

    await waitFor(() => expect(screen.getByText(/config.dbUsage.free/)).toBeInTheDocument());
    expect(screen.getByText(/900.0 Mo/)).toBeInTheDocument();
  });

  it('surfaces a failed measurement instead of showing an empty chart', async () => {
    configApi.dbUsage.mockRejectedValue(new Error('database is locked'));

    render(<DbUsageChart />);
    fireEvent.click(screen.getByRole('button', { name: /config.dbUsage.measure/ }));

    await waitFor(() => expect(screen.getByText('database is locked')).toBeInTheDocument());
    expect(screen.queryByRole('row')).not.toBeInTheDocument();
  });
});

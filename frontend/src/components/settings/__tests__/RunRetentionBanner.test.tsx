// The banner tells an install without run retention that its database only grows.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';

const { configApi } = vi.hoisted(() => ({
  configApi: { getServerConfig: vi.fn(), dbUsage: vi.fn() },
}));

vi.mock('../../../lib/api', () => ({ config: configApi }));
vi.mock('../../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: unknown[]) => (args.length ? `${key} ${args.join(' | ')}` : key),
    locale: 'en',
  }),
}));

import { RunRetentionBanner, RUN_RETENTION_BANNER_KEY } from '../RunRetentionBanner';
import { isSyncedKey } from '../../../lib/uiPreferences';

const GB = 1024 * 1024 * 1024;

beforeEach(() => {
  localStorage.removeItem(RUN_RETENTION_BANNER_KEY);
  configApi.getServerConfig.mockReset();
  configApi.dbUsage.mockReset();
  configApi.dbUsage.mockResolvedValue({ file_bytes: GB, wal_bytes: 0, free_bytes: 0, tables: [] });
});
afterEach(cleanup);

describe('RunRetentionBanner', () => {
  it('shows the database size and opens the setting while retention is off', async () => {
    const open = vi.fn();
    render(<RunRetentionBanner retentionDays={0} onOpenSetting={open} />);
    expect(screen.getByTestId('run-retention-banner')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText(/config.runRetentionBannerSize 1.00 Go/)).toBeInTheDocument());
    fireEvent.click(screen.getByText('config.runRetentionBannerOpen'));
    expect(open).toHaveBeenCalledTimes(1);
  });

  it('is hidden, and measures nothing, while retention is on', () => {
    render(<RunRetentionBanner retentionDays={30} onOpenSetting={vi.fn()} />);
    expect(screen.queryByTestId('run-retention-banner')).toBeNull();
    expect(configApi.dbUsage).not.toHaveBeenCalled();
  });

  it('reads the setting from the server when no value is given', async () => {
    configApi.getServerConfig.mockResolvedValue({ run_payload_retention_days: 0 });
    render(<RunRetentionBanner onOpenSetting={vi.fn()} />);
    expect(await screen.findByTestId('run-retention-banner')).toBeInTheDocument();
  });

  it('stays hidden once dismissed, and the dismissal is synced', async () => {
    const { rerender } = render(<RunRetentionBanner retentionDays={0} onOpenSetting={vi.fn()} />);
    await act(async () => { await Promise.resolve(); });
    fireEvent.click(screen.getByText('config.runRetentionBannerDismiss'));
    expect(screen.queryByTestId('run-retention-banner')).toBeNull();
    expect(localStorage.getItem(RUN_RETENTION_BANNER_KEY)).toBe('1');
    expect(isSyncedKey(RUN_RETENTION_BANNER_KEY, false)).toBe(true);
    rerender(<RunRetentionBanner retentionDays={0} onOpenSetting={vi.fn()} />);
    await act(async () => { await Promise.resolve(); });
    expect(screen.queryByTestId('run-retention-banner')).toBeNull();
  });

  it('comes back past 2 GB even when dismissed', async () => {
    localStorage.setItem(RUN_RETENTION_BANNER_KEY, '1');
    configApi.dbUsage.mockResolvedValue({ file_bytes: 3 * GB, wal_bytes: 0, free_bytes: 0, tables: [] });
    render(<RunRetentionBanner retentionDays={0} onOpenSetting={vi.fn()} />);
    expect(await screen.findByTestId('run-retention-banner')).toBeInTheDocument();
  });
});

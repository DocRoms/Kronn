/** KT-1007 — the app-wide key notices. */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent } from '@testing-library/react';

const { config } = vi.hoisted(() => ({
  config: { getRecoveryStatus: vi.fn(), restoreRecovery: vi.fn() },
}));
vi.mock('../../lib/api', () => ({ config }));

import { KeyLockedBanner } from '../KeyLockedBanner';

describe('KeyLockedBanner', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('says the stored credentials failed to load while the key is in use (C5-05)', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, credentials_unavailable: 'disk full' });
    render(<KeyLockedBanner />);
    expect(await screen.findByTestId('credentials-unavailable-banner')).toBeTruthy();
    expect(screen.queryByTestId('key-locked-banner')).toBeNull();
  });

  it('shows a dismissible notice for a config.toml set aside (C5-02)', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, config_set_aside: 'kept as config.toml.corrupt.1' });
    render(<KeyLockedBanner />);
    const banner = await screen.findByTestId('config-set-aside-banner');
    fireEvent.click(banner.querySelector('button')!);
    expect(screen.queryByTestId('config-set-aside-banner')).toBeNull();
  });
});

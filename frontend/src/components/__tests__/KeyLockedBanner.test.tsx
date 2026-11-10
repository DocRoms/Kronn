/** KT-1007 — the app-wide key notices. */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, fireEvent, waitFor, act } from '@testing-library/react';

const { config } = vi.hoisted(() => ({
  config: { getRecoveryStatus: vi.fn(), restoreRecovery: vi.fn() },
}));
vi.mock('../../lib/api', () => ({ config }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));

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
  // KT-1594
  it('flags a key stored beside its data and links to Settings > Recovery', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, config_holds_key: true });
    render(<KeyLockedBanner />);
    const banner = await screen.findByTestId('key-beside-data-banner');
    expect(banner.querySelector('a')!.getAttribute('href')).toBe('/config#settings-recovery');
  });

  it('says nothing about a key beside its data when config.toml does not hold it', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, config_holds_key: false });
    render(<KeyLockedBanner />);
    await waitFor(() => expect(config.getRecoveryStatus).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.queryByTestId('key-beside-data-banner')).toBeNull();
  });

  it('lists, by name, the credentials still in clear while the key is locked', async () => {
    config.getRecoveryStatus.mockResolvedValue({
      key_locked: true,
      clear_credentials: [{ kind: 'provider_key', name: 'Anthropic', provider: 'anthropic', verified: false }],
    });
    render(<KeyLockedBanner />);
    const list = await screen.findByTestId('key-locked-clear-credentials');
    expect(list.textContent).toContain('Anthropic');
  });
});

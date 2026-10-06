/**
 * KT-1007 — the auth-locked screen adapts to the caller and the state:
 * remote caller → "open Kronn on its machine"; key lost → restore form;
 * key in use but token row unreadable → set a new token.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';

const { config } = vi.hoisted(() => ({
  config: {
    getRecoveryStatus: vi.fn(),
    regenerateAuthToken: vi.fn(),
    restoreRecovery: vi.fn(),
  },
}));
vi.mock('../../lib/api', () => ({ config }));

import { AuthLockedScreen } from '../AuthLockedScreen';
import { ApiRequestError } from '../../lib/apiRequestError';

describe('AuthLockedScreen', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('tells a remote caller to open Kronn on its machine', async () => {
    config.getRecoveryStatus.mockRejectedValue(new ApiRequestError('locked', 'auth_locked'));
    render(<AuthLockedScreen onRestored={vi.fn()} />);
    await waitFor(() => expect(screen.getByTestId('auth-locked-remote')).toBeTruthy());
    expect(screen.queryByTestId('recovery-restore-panel')).toBeNull();
  });

  it('shows the restore form when the key is lost', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: true });
    render(<AuthLockedScreen onRestored={vi.fn()} />);
    await waitFor(() => expect(screen.getByTestId('recovery-restore-panel')).toBeTruthy());
  });

  it('sets a new token when only the token row is unreadable', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false });
    config.regenerateAuthToken.mockResolvedValue('new-token-123');
    const onRestored = vi.fn();
    render(<AuthLockedScreen onRestored={onRestored} />);
    fireEvent.click(await screen.findByTestId('auth-locked-new-token-btn'));
    await waitFor(() => expect(screen.getByTestId('auth-locked-new-token').textContent).toBe('new-token-123'));
    expect(screen.queryByTestId('recovery-restore-panel')).toBeNull();
  });

  it('says to fix the cause and restart when the credentials failed to load (C3-04)', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, credentials_unavailable: 'disk full' });
    render(<AuthLockedScreen onRestored={vi.fn()} />);
    await waitFor(() => expect(screen.getByTestId('auth-locked-restart').textContent).toContain('disk full'));
    expect(screen.queryByTestId('auth-locked-new-token-btn')).toBeNull();
  });

  it('retries, without the remote hint, when the status cannot be read (C5-09)', async () => {
    config.getRecoveryStatus.mockRejectedValueOnce(new TypeError('Failed to fetch'))
      .mockResolvedValueOnce({ key_locked: true });
    render(<AuthLockedScreen onRestored={vi.fn()} />);
    fireEvent.click(await screen.findByTestId('auth-locked-retry'));
    expect(screen.queryByTestId('auth-locked-remote')).toBeNull();
    await waitFor(() => expect(screen.getByTestId('recovery-restore-panel')).toBeTruthy());
    expect(config.getRecoveryStatus).toHaveBeenCalledTimes(2);
  });

  it('shows the restart state after a restore whose credentials failed (C5-04)', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, credentials_unavailable: 'write config backup: disk full' });
    render(<AuthLockedScreen onRestored={vi.fn()} />);
    await waitFor(() => expect(screen.getByTestId('auth-locked-restart')).toBeTruthy());
  });
});

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

describe('AuthLockedScreen', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('tells a remote caller to open Kronn on its machine', async () => {
    config.getRecoveryStatus.mockRejectedValue(new Error('locked'));
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
});

/**
 * P2 (2026-07) — recovery passphrase UI (RecoverySection).
 *
 * This section is the user-facing half of the anti-secret-loss hardening: it
 * wraps the encryption key under a passphrase and reveals the recovery code
 * ONCE. A regression here (code not shown, set not called, nudge missing)
 * silently leaves users without any total-loss recovery. Pins:
 *  - unconfigured → nudge shown; configured → badge shown
 *  - too-short / mismatched passphrases keep the save button disabled
 *  - save calls configApi.setRecovery and reveals the returned code exactly once
 *  - API failure surfaces the backend message via toast, no code block
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';

const { config } = vi.hoisted(() => ({
  config: {
    getRecoveryStatus: vi.fn(),
    setRecovery: vi.fn(),
    restoreRecovery: vi.fn(),
  },
}));

vi.mock('../../../lib/api', () => ({ config }));

import { RecoverySection } from '../RecoverySection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}:${args.join(',')}` : key;

describe('RecoverySection', () => {
  const toast = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    config.getRecoveryStatus.mockResolvedValue({ configured: false });
  });

  afterEach(() => cleanup());

  it('shows the nudge when no passphrase is configured', async () => {
    render(<RecoverySection toast={toast} t={t} />);
    await waitFor(() => expect(screen.getByTestId('recovery-nudge')).toBeTruthy());
    expect(screen.queryByTestId('recovery-configured-badge')).toBeNull();
  });

  it('shows the configured badge (and no nudge) when already set', async () => {
    config.getRecoveryStatus.mockResolvedValue({ configured: true });
    render(<RecoverySection toast={toast} t={t} />);
    await waitFor(() => expect(screen.getByTestId('recovery-configured-badge')).toBeTruthy());
    expect(screen.queryByTestId('recovery-nudge')).toBeNull();
  });

  it('keeps save disabled for a too-short or mismatched passphrase', async () => {
    render(<RecoverySection toast={toast} t={t} />);
    const save = await screen.findByTestId('recovery-save') as HTMLButtonElement;
    expect(save.disabled).toBe(true);

    fireEvent.change(screen.getByTestId('recovery-passphrase'), { target: { value: 'short' } });
    fireEvent.change(screen.getByTestId('recovery-confirm'), { target: { value: 'short' } });
    expect(save.disabled).toBe(true); // < 8 chars

    fireEvent.change(screen.getByTestId('recovery-passphrase'), { target: { value: 'long-enough-pass' } });
    fireEvent.change(screen.getByTestId('recovery-confirm'), { target: { value: 'different-pass' } });
    expect(save.disabled).toBe(true); // mismatch
  });

  it('saves and reveals the recovery code once', async () => {
    config.setRecovery.mockResolvedValue({ recovery_code: 'KRECOV1.abc.def' });
    render(<RecoverySection toast={toast} t={t} />);

    fireEvent.change(await screen.findByTestId('recovery-passphrase'), { target: { value: 'long-enough-pass' } });
    fireEvent.change(screen.getByTestId('recovery-confirm'), { target: { value: 'long-enough-pass' } });
    fireEvent.click(screen.getByTestId('recovery-save'));

    await waitFor(() => expect(screen.getByTestId('recovery-code')).toBeTruthy());
    expect(config.setRecovery).toHaveBeenCalledWith('long-enough-pass');
    expect(screen.getByTestId('recovery-code').textContent).toBe('KRECOV1.abc.def');
    expect(toast).toHaveBeenCalledWith('settings.recovery.saved', 'success');
  });

  it('replacing an existing passphrase requires and sends the current one', async () => {
    config.getRecoveryStatus.mockResolvedValue({ configured: true });
    config.setRecovery.mockResolvedValue({ recovery_code: 'KRECOV1.new.code' });
    render(<RecoverySection toast={toast} t={t} />);

    const current = await screen.findByTestId('recovery-current');
    fireEvent.change(screen.getByTestId('recovery-passphrase'), { target: { value: 'second-long-pass' } });
    fireEvent.change(screen.getByTestId('recovery-confirm'), { target: { value: 'second-long-pass' } });
    const save = screen.getByTestId('recovery-save') as HTMLButtonElement;
    expect(save.disabled).toBe(true); // current passphrase missing

    fireEvent.change(current, { target: { value: 'first-long-pass' } });
    expect(save.disabled).toBe(false);
    fireEvent.click(save);

    await waitFor(() => expect(screen.getByTestId('recovery-code')).toBeTruthy());
    expect(config.setRecovery).toHaveBeenCalledWith('second-long-pass', 'first-long-pass');
  });

  it('does not ask for a current passphrase on first set', async () => {
    render(<RecoverySection toast={toast} t={t} />);
    await screen.findByTestId('recovery-passphrase');
    expect(screen.queryByTestId('recovery-current')).toBeNull();
  });

  it('surfaces a backend error via toast and shows no code', async () => {
    config.setRecovery.mockRejectedValue(new Error('no active encryption key'));
    render(<RecoverySection toast={toast} t={t} />);

    fireEvent.change(await screen.findByTestId('recovery-passphrase'), { target: { value: 'long-enough-pass' } });
    fireEvent.change(screen.getByTestId('recovery-confirm'), { target: { value: 'long-enough-pass' } });
    fireEvent.click(screen.getByTestId('recovery-save'));

    await waitFor(() => expect(toast).toHaveBeenCalledWith('no active encryption key', 'error'));
    expect(screen.queryByTestId('recovery-code-block')).toBeNull();
  });

  const fullStatus = (over: Record<string, unknown>) => ({
    configured: true, matches_key: true, key_locked: false, key_copies_kept: false,
    config_holds_key: false, copies: 2, stale_sources: [], invalid_sources: [],
    locked_credentials: 0, kept_recovery_blobs: 0, recovery_other_key: false, config_set_aside: null, ...over,
  });

  it('replaces a recovery.key for another key without asking its passphrase', async () => {
    config.getRecoveryStatus.mockResolvedValue(fullStatus({ matches_key: false, recovery_other_key: true }));
    config.setRecovery.mockResolvedValue({ recovery_code: 'KRECOV1.new' });
    render(<RecoverySection toast={toast} t={t} />);
    fireEvent.change(await screen.findByTestId('recovery-passphrase'), { target: { value: 'long-enough-pass' } });
    fireEvent.change(screen.getByTestId('recovery-confirm'), { target: { value: 'long-enough-pass' } });
    expect(screen.queryByTestId('recovery-current')).toBeNull();
    fireEvent.click(screen.getByTestId('recovery-save'));
    await waitFor(() => expect(config.setRecovery).toHaveBeenCalledWith('long-enough-pass'));
  });

  it('still asks the current passphrase for a recovery.key it cannot verify', async () => {
    config.getRecoveryStatus.mockResolvedValue(fullStatus({ matches_key: false, recovery_other_key: false }));
    render(<RecoverySection toast={toast} t={t} />);
    expect(await screen.findByTestId('recovery-current')).toBeTruthy();
  });

  it('offers the restore form, not the set form, when the key is locked', async () => {
    config.getRecoveryStatus.mockResolvedValue(fullStatus({ key_locked: true }));
    render(<RecoverySection toast={toast} t={t} />);
    await waitFor(() => expect(screen.getByTestId('recovery-locked')).toBeTruthy());
    expect(screen.getByTestId('recovery-restore-panel')).toBeTruthy();
    expect(screen.queryByTestId('recovery-save')).toBeNull();
  });

  it('warns when recovery.key does not protect the key in use, without the configured badge', async () => {
    config.getRecoveryStatus.mockResolvedValue(fullStatus({ matches_key: false }));
    render(<RecoverySection toast={toast} t={t} />);
    await waitFor(() => expect(screen.getByTestId('recovery-mismatch')).toBeTruthy());
    expect(screen.queryByTestId('recovery-configured-badge')).toBeNull();
  });

  it('names a key store holding another key', async () => {
    config.getRecoveryStatus.mockResolvedValue(fullStatus({ stale_sources: ['keychain'] }));
    render(<RecoverySection toast={toast} t={t} />);
    await waitFor(() => expect(screen.getByTestId('recovery-stale').textContent).toContain('keychain'));
  });

  it('shows the computed warnings: invalid store, single copy, locked credentials (C3-11)', async () => {
    config.getRecoveryStatus.mockResolvedValue(fullStatus({
      matches_key: false, copies: 1, invalid_sources: ['sidecar'], locked_credentials: 2,
    }));
    render(<RecoverySection toast={toast} t={t} />);
    await waitFor(() => expect(screen.getByTestId('recovery-invalid').textContent).toContain('sidecar'));
    expect(screen.getByTestId('recovery-single-copy')).toBeTruthy();
    expect(screen.getByTestId('recovery-locked-credentials').textContent).toContain('2');
  });
});

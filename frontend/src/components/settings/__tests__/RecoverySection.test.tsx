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

  it('surfaces a backend error via toast and shows no code', async () => {
    config.setRecovery.mockRejectedValue(new Error('no active encryption key'));
    render(<RecoverySection toast={toast} t={t} />);

    fireEvent.change(await screen.findByTestId('recovery-passphrase'), { target: { value: 'long-enough-pass' } });
    fireEvent.change(screen.getByTestId('recovery-confirm'), { target: { value: 'long-enough-pass' } });
    fireEvent.click(screen.getByTestId('recovery-save'));

    await waitFor(() => expect(toast).toHaveBeenCalledWith('no active encryption key', 'error'));
    expect(screen.queryByTestId('recovery-code-block')).toBeNull();
  });
});

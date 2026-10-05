/**
 * P2 (2026-07) — inline key-restore flow (RecoveryRestorePanel), shown in the
 * plugins "not operational" banner. Pins:
 *  - collapsed CTA by default; expands on click
 *  - restore calls the API with passphrase (+ trimmed code when provided,
 *    undefined when blank) and fires onRestored on success
 *  - backend error message is surfaced verbatim; onRestored NOT called
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';

const { config } = vi.hoisted(() => ({
  config: {
    restoreRecovery: vi.fn(),
    reencryptImported: vi.fn(),
    getRecoveryStatus: vi.fn(),
  },
}));

vi.mock('../../lib/api', () => ({ config }));

import { RecoveryRestorePanel } from '../RecoveryRestorePanel';

const t = (key: string) => key;

describe('RecoveryRestorePanel', () => {
  const toast = vi.fn();
  const onRestored = vi.fn();

  beforeEach(() => {
    vi.clearAllMocks();
    // Locked instance by default: the restore flow.
    config.getRecoveryStatus.mockResolvedValue({ key_locked: true });
  });
  afterEach(() => cleanup());

  it('renders collapsed and expands on click', () => {
    render(<RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} />);
    expect(screen.queryByTestId('recovery-restore-panel')).toBeNull();
    fireEvent.click(screen.getByTestId('recovery-restore-cta'));
    expect(screen.getByTestId('recovery-restore-panel')).toBeTruthy();
  });

  it('restores with passphrase only (blank code → undefined) and refetches', async () => {
    config.restoreRecovery.mockResolvedValue(undefined);
    render(<RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} />);
    fireEvent.click(screen.getByTestId('recovery-restore-cta'));
    fireEvent.change(screen.getByTestId('recovery-restore-passphrase'), { target: { value: 'my-pass' } });
    await waitFor(() => expect((screen.getByTestId('recovery-restore-submit') as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByTestId('recovery-restore-submit'));

    await waitFor(() => expect(onRestored).toHaveBeenCalled());
    expect(config.restoreRecovery).toHaveBeenCalledWith('my-pass', undefined);
    expect(toast).toHaveBeenCalledWith('mcp.recovery.restored', 'success');
  });

  it('passes a trimmed recovery code when provided', async () => {
    config.restoreRecovery.mockResolvedValue(undefined);
    render(<RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} />);
    fireEvent.click(screen.getByTestId('recovery-restore-cta'));
    fireEvent.change(screen.getByTestId('recovery-restore-passphrase'), { target: { value: 'my-pass' } });
    fireEvent.change(screen.getByTestId('recovery-restore-code'), { target: { value: '  KRECOV1.a.b  ' } });
    await waitFor(() => expect((screen.getByTestId('recovery-restore-submit') as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByTestId('recovery-restore-submit'));

    await waitFor(() => expect(config.restoreRecovery).toHaveBeenCalledWith('my-pass', 'KRECOV1.a.b'));
  });

  it('surfaces the backend error verbatim and does not refetch', async () => {
    config.restoreRecovery.mockRejectedValue(new Error('Wrong recovery passphrase or corrupt recovery data'));
    render(<RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} />);
    fireEvent.click(screen.getByTestId('recovery-restore-cta'));
    fireEvent.change(screen.getByTestId('recovery-restore-passphrase'), { target: { value: 'bad' } });
    await waitFor(() => expect((screen.getByTestId('recovery-restore-submit') as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByTestId('recovery-restore-submit'));

    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith('Wrong recovery passphrase or corrupt recovery data', 'error'));
    expect(onRestored).not.toHaveBeenCalled();
  });

  it('on a running instance re-encrypts imported secrets instead of swapping the key', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false });
    config.reencryptImported.mockResolvedValue({ rewritten: 2, already_current: 5, untouched: 0 });
    render(<RecoveryRestorePanel toast={toast} t={(k: string, ...a: (string | number)[]) => a.length ? `${k}:${a.join(',')}` : k} onRestored={onRestored} />);
    await waitFor(() => expect(screen.getByText('mcp.recovery.reencryptCta')).toBeTruthy());
    fireEvent.click(screen.getByTestId('recovery-restore-cta'));
    fireEvent.change(screen.getByTestId('recovery-restore-passphrase'), { target: { value: 'source pass' } });
    await waitFor(() => expect((screen.getByTestId('recovery-restore-submit') as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByTestId('recovery-restore-submit'));
    await waitFor(() => expect(config.reencryptImported).toHaveBeenCalledWith('source pass', undefined));
    expect(config.restoreRecovery).not.toHaveBeenCalled();
    await waitFor(() => expect(toast).toHaveBeenCalledWith('mcp.recovery.reencrypted:2', 'success'));
    expect(onRestored).toHaveBeenCalled();
  });

  it('mode restore never asks the backend and starts open when asked', async () => {
    render(<RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} mode="restore" initiallyOpen />);
    expect(screen.getByTestId('recovery-restore-panel')).toBeTruthy();
    expect(config.getRecoveryStatus).not.toHaveBeenCalled();
  });

  it('keeps submit disabled until the backend says which flow applies', async () => {
    let answer: (v: unknown) => void = () => {};
    config.getRecoveryStatus.mockReturnValue(new Promise(resolve => { answer = resolve; }));
    render(<RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} initiallyOpen />);
    fireEvent.change(screen.getByTestId('recovery-restore-passphrase'), { target: { value: 'pw' } });
    const submit = screen.getByTestId('recovery-restore-submit') as HTMLButtonElement;
    expect(submit.disabled).toBe(true);
    fireEvent.click(submit);
    expect(config.restoreRecovery).not.toHaveBeenCalled();
    answer({ key_locked: false });
    await waitFor(() => expect(submit.disabled).toBe(false));
  });
});

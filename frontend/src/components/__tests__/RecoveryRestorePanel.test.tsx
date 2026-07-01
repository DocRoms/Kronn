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
  },
}));

vi.mock('../../lib/api', () => ({ config }));

import { RecoveryRestorePanel } from '../RecoveryRestorePanel';

const t = (key: string) => key;

describe('RecoveryRestorePanel', () => {
  const toast = vi.fn();
  const onRestored = vi.fn();

  beforeEach(() => vi.clearAllMocks());
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
    fireEvent.click(screen.getByTestId('recovery-restore-submit'));

    await waitFor(() => expect(config.restoreRecovery).toHaveBeenCalledWith('my-pass', 'KRECOV1.a.b'));
  });

  it('surfaces the backend error verbatim and does not refetch', async () => {
    config.restoreRecovery.mockRejectedValue(new Error('Wrong recovery passphrase or corrupt recovery data'));
    render(<RecoveryRestorePanel toast={toast} t={t} onRestored={onRestored} />);
    fireEvent.click(screen.getByTestId('recovery-restore-cta'));
    fireEvent.change(screen.getByTestId('recovery-restore-passphrase'), { target: { value: 'bad' } });
    fireEvent.click(screen.getByTestId('recovery-restore-submit'));

    await waitFor(() =>
      expect(toast).toHaveBeenCalledWith('Wrong recovery passphrase or corrupt recovery data', 'error'));
    expect(onRestored).not.toHaveBeenCalled();
  });
});

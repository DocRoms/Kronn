/**
 * KT-1492 — enrolling the authenticator that raising a trust level needs: a QR
 * code and the manual key, a first code to confirm, ten recovery codes shown
 * once; changing or disabling it needs a proof of the current one.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { TrustProofView } from '../../types/generated';

const { trustMock } = vi.hoisted(() => ({
  trustMock: {
    proof: vi.fn(),
    enrolProof: vi.fn(),
    confirmProof: vi.fn(),
    renewRecoveryCodes: vi.fn(),
    disableProof: vi.fn(),
  },
}));
vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<Record<string, object>>('../../lib/api');
  return { ...real, trust: trustMock };
});

import { I18nProvider } from '../../lib/I18nContext';
import { TrustProofSection } from '../settings/TrustProofSection';

const NONE: TrustProofView = { enrolled: false, pending: false, recovery_codes_left: 0, locked_for_secs: 0 };
const ENROLLED: TrustProofView = { enrolled: true, pending: false, recovery_codes_left: 9, locked_for_secs: 0 };
const ENROLMENT = {
  secret: 'GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ',
  uri: 'otpauth://totp/Kronn:human?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=Kronn&algorithm=SHA1&digits=6&period=30',
};
const CODES = Array.from({ length: 10 }, (_, i) => `abcd-efgh-ij${i}k`);

const wrap = () => render(<I18nProvider><TrustProofSection /></I18nProvider>);

beforeEach(() => {
  Object.values(trustMock).forEach(mock => mock.mockReset());
});
afterEach(cleanup);

describe('TrustProofSection', () => {
  it('enrols: shows the QR code and the manual key, confirms the first code, then shows the recovery codes once', async () => {
    trustMock.proof.mockResolvedValueOnce(NONE).mockResolvedValue(ENROLLED);
    trustMock.enrolProof.mockResolvedValue(ENROLMENT);
    trustMock.confirmProof.mockResolvedValue({ codes: CODES });
    wrap();

    // The first enrolment is authorised by the admin secret: nothing starts without it.
    const begin = await screen.findByTestId('trust-proof-begin');
    expect(begin).toBeDisabled();
    fireEvent.change(screen.getByTestId('trust-proof-admin-secret'), { target: { value: ' kr-admin-secret ' } });
    fireEvent.click(begin);
    await waitFor(() => expect(trustMock.enrolProof).toHaveBeenCalledWith(undefined, 'kr-admin-secret'));
    expect(await screen.findByTestId('trust-proof-qr')).toBeTruthy();
    expect(screen.getByTestId('trust-proof-qr').querySelectorAll('path')).toHaveLength(1);
    expect(screen.getByTestId('trust-proof-secret').textContent).toBe('GEZD GNBV GY3T QOJQ GEZD GNBV GY3T QOJQ');

    const confirm = screen.getByTestId('trust-proof-confirm');
    expect(confirm).toBeDisabled();
    fireEvent.change(screen.getByTestId('trust-proof-code'), { target: { value: ' 482913 ' } });
    fireEvent.click(confirm);
    await waitFor(() => expect(trustMock.confirmProof).toHaveBeenCalledWith('482913', 'kr-admin-secret'));

    const recovery = await screen.findByTestId('trust-proof-recovery');
    expect(recovery.querySelectorAll('li')).toHaveLength(10);
    fireEvent.click(screen.getByTestId('trust-proof-saved'));
    expect(screen.queryByTestId('trust-proof-recovery')).toBeNull();
    expect(screen.queryByTestId('trust-proof-qr')).toBeNull();
  });

  it('lets the owner cancel a pending enrolment with the admin secret', async () => {
    trustMock.proof.mockResolvedValueOnce({ ...NONE, pending: true }).mockResolvedValue(NONE);
    trustMock.disableProof.mockResolvedValue(NONE);
    wrap();

    const cancel = await screen.findByTestId('trust-proof-cancel-pending');
    expect(cancel).toBeDisabled();
    fireEvent.change(screen.getByTestId('trust-proof-admin-secret'), { target: { value: ' kr-admin-secret ' } });
    fireEvent.click(cancel);
    await waitFor(() => expect(trustMock.disableProof).toHaveBeenCalledWith(undefined, 'kr-admin-secret'));
    await waitFor(() => expect(screen.queryByTestId('trust-proof-cancel-pending')).toBeNull());
  });

  it('says nothing can be raised while no authenticator is enrolled', async () => {
    trustMock.proof.mockResolvedValue(NONE);
    wrap();
    expect((await screen.findByTestId('trust-proof-status')).textContent).toMatch(/aucun niveau/i);
    expect(screen.queryByTestId('trust-proof-disable')).toBeNull();
  });

  it('asks for a proof before it renews the codes, replaces or disables the enrolment', async () => {
    trustMock.proof.mockResolvedValue(ENROLLED);
    trustMock.renewRecoveryCodes.mockResolvedValue({ codes: CODES });
    trustMock.disableProof.mockResolvedValue(NONE);
    trustMock.enrolProof.mockResolvedValue(ENROLMENT);
    wrap();

    expect((await screen.findByTestId('trust-proof-status')).textContent).toContain('9');
    expect(screen.queryByTestId('trust-proof-admin-secret')).toBeNull();
    expect(screen.getByTestId('trust-proof-renew')).toBeDisabled();
    expect(screen.getByTestId('trust-proof-disable')).toBeDisabled();
    fireEvent.change(screen.getByTestId('trust-proof-action-code'), { target: { value: '123456' } });

    fireEvent.click(screen.getByTestId('trust-proof-renew'));
    await waitFor(() => expect(trustMock.renewRecoveryCodes).toHaveBeenCalledWith('123456'));
    expect(await screen.findByTestId('trust-proof-recovery')).toBeTruthy();
    fireEvent.click(screen.getByTestId('trust-proof-saved'));

    fireEvent.change(screen.getByTestId('trust-proof-action-code'), { target: { value: 'abcd-efgh-ij0k' } });
    fireEvent.click(screen.getByTestId('trust-proof-disable'));
    await waitFor(() => expect(trustMock.disableProof).toHaveBeenCalledWith('abcd-efgh-ij0k'));
  });

  it('replaces the authenticator with a proof of the current one', async () => {
    trustMock.proof.mockResolvedValue(ENROLLED);
    trustMock.enrolProof.mockResolvedValue(ENROLMENT);
    wrap();
    await screen.findByTestId('trust-proof-status');
    fireEvent.change(screen.getByTestId('trust-proof-action-code'), { target: { value: '654321' } });
    // Once enrolled, the admin secret is not asked until a replacement starts.
    expect(screen.queryByTestId('trust-proof-admin-secret')).toBeNull();
    fireEvent.click(screen.getByTestId('trust-proof-replace-open'));
    // The current code is not enough: a replacement is authorised by the admin secret too.
    expect(screen.getByTestId('trust-proof-replace')).toBeDisabled();
    fireEvent.change(screen.getByTestId('trust-proof-admin-secret'), { target: { value: 'kr-admin-secret' } });
    fireEvent.click(screen.getByTestId('trust-proof-replace'));
    await waitFor(() => expect(trustMock.enrolProof).toHaveBeenCalledWith('654321', 'kr-admin-secret'));
    expect(await screen.findByTestId('trust-proof-qr')).toBeTruthy();
  });

  it('leaves the replacement with a cancel, and words the admin-secret help for a replacement', async () => {
    trustMock.proof.mockResolvedValue(ENROLLED);
    wrap();
    await screen.findByTestId('trust-proof-status');
    fireEvent.click(screen.getByTestId('trust-proof-replace-open'));
    const help = screen.getByTestId('trust-proof-admin-secret').closest('label')!;
    expect(help.textContent).toContain('Pour remplacer');
    expect(help.textContent).not.toContain('Nécessaire au premier enrôlement');
    fireEvent.change(screen.getByTestId('trust-proof-admin-secret'), { target: { value: 'kr-admin-secret' } });
    fireEvent.click(screen.getByTestId('trust-proof-replace-cancel'));
    expect(screen.queryByTestId('trust-proof-admin-secret')).toBeNull();
    expect(screen.getByTestId('trust-proof-replace-open')).toBeTruthy();
    expect(trustMock.enrolProof).not.toHaveBeenCalled();
  });

  it('says the delay while the confirmation is locked, and still takes a recovery code', async () => {
    trustMock.proof.mockResolvedValue({ ...ENROLLED, locked_for_secs: 37 });
    trustMock.disableProof.mockResolvedValue(undefined);
    wrap();
    expect((await screen.findByTestId('trust-proof-locked')).textContent).toContain('37');
    // A recovery code is the way out of a lockout: the actions stay available.
    expect(screen.getByTestId('trust-proof-replace-open')).toBeEnabled();
    fireEvent.change(screen.getByTestId('trust-proof-action-code'), { target: { value: 'abcd-efgh-ijkl' } });
    expect(screen.getByTestId('trust-proof-renew')).toBeEnabled();
    const disable = screen.getByTestId('trust-proof-disable');
    expect(disable).toBeEnabled();
    fireEvent.click(disable);
    await waitFor(() => expect(trustMock.disableProof).toHaveBeenCalledWith('abcd-efgh-ijkl'));
  });

  it('tells how to reset it from the host', async () => {
    trustMock.proof.mockResolvedValue(NONE);
    const { container } = wrap();
    await screen.findByTestId('trust-proof-status');
    expect(container.textContent).toContain('trust-proof.json');
    expect(container.textContent).toContain('trust-proof-lock.json');
  });

  it('says where the admin secret is and that an agent cannot read it', async () => {
    trustMock.proof.mockResolvedValue(NONE);
    const { container } = wrap();
    await screen.findByTestId('trust-proof-status');
    expect(container.textContent).toContain('human-admin-secret');
    expect(screen.getByTestId('trust-proof-admin-secret')).toHaveAttribute('type', 'password');
  });

  it('shows nothing, rather than breaking Settings, when the state cannot be read', async () => {
    trustMock.proof.mockRejectedValue(new Error('down'));
    const { container } = wrap();
    await waitFor(() => expect(trustMock.proof).toHaveBeenCalled());
    expect(container.querySelector('[data-testid="trust-proof-section"]')).toBeNull();
  });
});

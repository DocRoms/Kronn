/**
 * KT-1492 — enrolling the authenticator that raising a trust level needs: a QR
 * code and the manual key, a first code to confirm, ten recovery codes shown
 * once; changing or disabling it needs a proof of the current one.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { TrustOverview, TrustProofView } from '../../types/generated';

const { trustMock } = vi.hoisted(() => ({
  trustMock: {
    overview: vi.fn(),
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

const PATHS = {
  admin_secret_path: '/Users/romu/Library/Application Support/com.kronn.kronn/human-admin-secret',
  data_dir: '/Users/romu/Library/Application Support/com.kronn.kronn',
  in_docker: false,
};
const NONE: TrustProofView = { enrolled: false, pending: false, recovery_codes_left: 0, locked_for_secs: 0, ...PATHS };
const ENROLLED: TrustProofView = { enrolled: true, pending: false, recovery_codes_left: 9, locked_for_secs: 0, ...PATHS };
const PROTECTED: TrustOverview['sandbox'] = { available: true, protect_vault: true, protected: true };
/** The overview the card reads: the proof state and the vault protection. */
const overview = (proof: TrustProofView, sandbox = PROTECTED) => ({ proof, sandbox }) as TrustOverview;
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
    trustMock.overview.mockResolvedValueOnce(overview(NONE)).mockResolvedValue(overview(ENROLLED));
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
    trustMock.overview.mockResolvedValueOnce(overview({ ...NONE, pending: true })).mockResolvedValue(overview(NONE));
    trustMock.disableProof.mockResolvedValue(NONE);
    wrap();

    const cancel = await screen.findByTestId('trust-proof-cancel-pending');
    expect(cancel).toBeDisabled();
    fireEvent.change(screen.getByTestId('trust-proof-admin-secret'), { target: { value: ' kr-admin-secret ' } });
    fireEvent.click(cancel);
    await waitFor(() => expect(trustMock.disableProof).toHaveBeenCalledWith(undefined, 'kr-admin-secret'));
    await waitFor(() => expect(screen.queryByTestId('trust-proof-cancel-pending')).toBeNull());
  });

  it('says the agents stay at Standard while no authenticator is enrolled, and badges it', async () => {
    trustMock.overview.mockResolvedValue(overview(NONE));
    const { container } = wrap();
    expect((await screen.findByTestId('trust-proof-status')).textContent).toMatch(/restent au niveau Standard/);
    expect(screen.getByTestId('trust-proof-badge')).toHaveAttribute('data-on', 'false');
    expect(screen.getByTestId('trust-proof-badge').textContent).toBe('Non configuré');
    expect(screen.queryByTestId('trust-proof-disable')).toBeNull();
    expect(container.querySelector('#settings-authenticator.set-card')).not.toBeNull();
    // The vault is protected here: no warning that agents can read the file.
    expect(screen.queryByTestId('trust-proof-unprotected')).toBeNull();
  });

  it('badges a pending enrolment and an active one with its backup codes left', async () => {
    trustMock.overview.mockResolvedValue(overview({ ...NONE, pending: true }));
    wrap();
    expect(await screen.findByTestId('trust-proof-badge')).toHaveAttribute('data-on', 'pending');
    expect(screen.getByTestId('trust-proof-badge').textContent).toBe('En attente');
    cleanup();

    trustMock.overview.mockResolvedValue(overview(ENROLLED));
    wrap();
    const badge = await screen.findByTestId('trust-proof-badge');
    expect(badge).toHaveAttribute('data-on', 'true');
    expect(badge.textContent).toBe('Actif · 9 codes de secours');
  });

  it('warns that agents can read the admin secret too when the data folder is not protected', async () => {
    trustMock.overview.mockResolvedValue(overview(NONE, { available: false, protect_vault: true, protected: false }));
    wrap();
    expect((await screen.findByTestId('trust-proof-unprotected')).textContent).toMatch(/tes agents peuvent aussi lire ce fichier/);
  });

  it('asks for a proof before it renews the codes, replaces or disables the enrolment', async () => {
    trustMock.overview.mockResolvedValue(overview(ENROLLED));
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
    trustMock.overview.mockResolvedValue(overview(ENROLLED));
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
    trustMock.overview.mockResolvedValue(overview(ENROLLED));
    wrap();
    await screen.findByTestId('trust-proof-status');
    fireEvent.click(screen.getByTestId('trust-proof-replace-open'));
    const help = screen.getByTestId('trust-proof-admin-hint');
    expect(help.textContent).toContain('Pour remplacer');
    expect(help.textContent).toContain(`cat "${ENROLLED.admin_secret_path}"`);
    // Where the secret came from, and why the current code is needed as well.
    expect(help.textContent).toContain('au premier démarrage, ou à la dernière récupération');
    expect(help.textContent).toContain('le code actuel que tu détiens');
    fireEvent.change(screen.getByTestId('trust-proof-admin-secret'), { target: { value: 'kr-admin-secret' } });
    fireEvent.click(screen.getByTestId('trust-proof-replace-cancel'));
    expect(screen.queryByTestId('trust-proof-admin-secret')).toBeNull();
    expect(screen.getByTestId('trust-proof-replace-open')).toBeTruthy();
    expect(trustMock.enrolProof).not.toHaveBeenCalled();
  });

  it('says the delay while the confirmation is locked, and still takes a recovery code', async () => {
    trustMock.overview.mockResolvedValue(overview({ ...ENROLLED, locked_for_secs: 37 }));
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

  it('tells how to reset it from the host, in the data folder the server names', async () => {
    trustMock.overview.mockResolvedValue(overview(ENROLLED));
    wrap();
    const reset = await screen.findByTestId('trust-proof-reset');
    expect(reset.textContent).toContain('trust-proof.json');
    expect(reset.textContent).toContain('trust-proof-lock.json');
    expect(reset.textContent).toContain(ENROLLED.data_dir);
  });

  it('shows the admin secret path from the payload, with a copy button, and how to get a new secret', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
    trustMock.overview.mockResolvedValue(overview(NONE));
    wrap();
    const hint = await screen.findByTestId('trust-proof-admin-hint');
    expect(hint.textContent).toContain(`cat "${NONE.admin_secret_path}"`);
    // Never the Linux path guessed on every platform.
    expect(hint.textContent).not.toContain('~/.config/kronn');
    expect(screen.getByTestId('trust-proof-admin-secret-path').textContent).toBe(`cat "${NONE.admin_secret_path}"`);
    fireEvent.click(within(hint).getByRole('button'));
    expect(writeText).toHaveBeenCalledWith(`cat "${NONE.admin_secret_path}"`);
    expect(screen.getByTestId('trust-proof-admin-secret')).toHaveAttribute('type', 'password');
    expect(screen.getByTestId('trust-proof-lost').textContent).toContain('recover-admin-secret');
    expect(screen.getByTestId('trust-proof-lost').textContent).toContain(NONE.data_dir);
  });

  it('names the payload path in both hints, says where the secret came from and what the sandbox covers', async () => {
    // A KRONN_DATA_DIR install: neither default folder, only the server knows it.
    const custom = { admin_secret_path: '/srv/kronn-data/human-admin-secret', data_dir: '/srv/kronn-data' };
    trustMock.overview.mockResolvedValueOnce(overview({ ...NONE, ...custom })).mockResolvedValue(overview({ ...ENROLLED, ...custom }));
    const first = wrap();
    const hint = await screen.findByTestId('trust-proof-admin-hint');
    expect(hint.textContent).toContain('cat "/srv/kronn-data/human-admin-secret"');
    expect(hint.textContent).toContain('au premier démarrage, ou à la dernière récupération');
    expect(hint.textContent).toContain('faire tourner un secret avec lui et remplacer l\'authentificateur demandent aussi son code ; lister les identifiants, non');
    expect(hint.textContent).toContain('dernière récupération ou rotation');
    expect(hint.textContent).not.toContain('seulement pour le premier enrôlement');
    const who = screen.getByText(/Seul ton compte utilisateur/);
    expect(who.textContent).toContain('tant que sa protection est active');
    expect(who.textContent).toContain('une CLI que tu lances toi-même');
    expect(who.textContent).toContain('avant de lui confier des tâches');
    first.unmount();

    wrap();
    fireEvent.click(await screen.findByTestId('trust-proof-replace-open'));
    expect(screen.getByTestId('trust-proof-admin-hint').textContent).toContain('cat "/srv/kronn-data/human-admin-secret"');
  });

  it('words the admin-secret hints the same way in the 4 locales', async () => {
    const { dictionaries } = await import('../../lib/i18n/testing');
    for (const [locale, dict] of Object.entries(dictionaries)) {
      const text = dict as Record<string, string>;
      for (const key of ['trust.proof.adminSecretHint', 'trust.proof.adminSecretHintReplace', 'trust.proof.adminSecretHintDocker']) {
        expect(text[key], `${locale} ${key}`).toContain('{0}');
        expect(text[key], `${locale} ${key}`).not.toContain('.config/kronn');
      }
      // Same structure everywhere: the first-start sentence, then the sandbox scope with the CLI caveat.
      expect(text['trust.proof.adminSecretHint'].split('{0}')[0].length, locale).toBeGreaterThan(40);
      expect(text['trust.proof.adminSecretWho'], locale).toMatch(/CLI/);
    }
    expect(dictionaries.en['trust.proof.adminSecretHint']).toContain('at first start, or at the last recovery');
    expect(dictionaries.en['trust.proof.adminSecretHint']).toContain("also need the authenticator's code");
    expect(dictionaries.en['settings.credentials.intro']).toContain("authenticator's code once one is enrolled");
    expect(dictionaries.en['trust.proof.adminSecretWho']).toContain('while its protection is on');
    expect(dictionaries.en['trust.proof.adminSecretWho']).not.toMatch(/cannot see it/);
    expect(dictionaries.es['trust.proof.adminSecretWho']).toContain('mientras su protección esté activa');
    expect(dictionaries.zh['trust.proof.adminSecretWho']).toContain('保护开启期间');
  });

  it('points inside the container under Docker', async () => {
    trustMock.overview.mockResolvedValue(overview({
      ...NONE, in_docker: true, admin_secret_path: '/data/human-admin-secret', data_dir: '/data',
    }));
    wrap();
    const hint = await screen.findByTestId('trust-proof-admin-hint');
    expect(hint.textContent).toMatch(/dans le conteneur Kronn/);
    expect(hint.textContent).toContain('/data/human-admin-secret');
    expect(hint.textContent).not.toContain('cat "');
  });

  it('gives a PowerShell command for a Windows path', async () => {
    const path = 'C:\\Users\\romu\\AppData\\Roaming\\kronn\\kronn\\config\\human-admin-secret';
    trustMock.overview.mockResolvedValue(overview({ ...NONE, admin_secret_path: path }));
    wrap();
    expect((await screen.findByTestId('trust-proof-admin-secret-path')).textContent).toBe(`Get-Content "${path}"`);
  });

  it('keeps a generic file name when the server could not resolve the path', async () => {
    trustMock.overview.mockResolvedValue(overview({ ...NONE, admin_secret_path: '', data_dir: '' }));
    wrap();
    const hint = await screen.findByTestId('trust-proof-admin-hint');
    expect(hint.textContent).toContain('human-admin-secret');
    expect(screen.getByTestId('trust-proof-admin-command-mac').textContent)
      .toBe('cat "$HOME/Library/Application Support/com.kronn.kronn/human-admin-secret"');
    expect(screen.queryByTestId('trust-proof-admin-secret-path')).toBeNull();
    expect(screen.queryByTestId('trust-proof-lost')).toBeNull();
  });

  it('names the usual folders, never a bare file name as a command, when an older backend sends no path', async () => {
    const { admin_secret_path: _path, data_dir: _dir, in_docker: _docker, ...older } = NONE;
    trustMock.overview.mockResolvedValue(overview(older as typeof NONE));
    wrap();
    const hint = await screen.findByTestId('trust-proof-admin-hint');
    expect(screen.getByTestId('trust-proof-admin-command-linux').textContent)
      .toBe('cat "$HOME/.config/kronn/human-admin-secret"');
    expect(hint.textContent).not.toMatch(/terminal : human-admin-secret/);
    expect(screen.getByTestId('trust-proof-admin-command-windows').textContent)
      .toBe('Get-Content "$env:APPDATA\\kronn\\kronn\\config\\human-admin-secret"');
    expect(screen.queryByTestId('trust-proof-admin-secret-path')).toBeNull();
  });

  it('shows nothing, rather than breaking Settings, when the state cannot be read', async () => {
    trustMock.overview.mockRejectedValue(new Error('down'));
    const { container } = wrap();
    await waitFor(() => expect(trustMock.overview).toHaveBeenCalled());
    expect(container.querySelector('[data-testid="trust-proof-section"]')).toBeNull();
  });
});

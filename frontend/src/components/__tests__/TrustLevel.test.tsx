/**
 * KT-1492 — choosing a trust level above Standard goes through a risk panel
 * that lists the concrete effects and needs an explicit confirmation; an
 * active level shows a badge and is revoked in one click.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type {
  EffectiveTrust, TrustLevel, TrustOverview, TrustScope, TrustScopeView,
} from '../../types/generated';

const { trustMock } = vi.hoisted(() => ({
  trustMock: {
    overview: vi.fn(),
    audit: vi.fn(),
    instance: vi.fn(),
    setInstance: vi.fn(),
    get: vi.fn(),
    set: vi.fn(),
    clear: vi.fn(),
  },
}));
vi.mock('../../lib/api', async () => {
  const real = await vi.importActual<Record<string, object>>('../../lib/api');
  return { ...real, trust: trustMock };
});

import { I18nProvider } from '../../lib/I18nContext';
import { TrustBadge, TrustLevelControl } from '../TrustLevel';

const OVERVIEW: TrustOverview = {
  instance: { scope: 'instance', level: 'standard' },
  effects: [
    { approval: 'quick_exec_line', from_level: 'trusted', enforced: true },
    { approval: 'exec_step_line', from_level: 'trusted', enforced: true },
    { approval: 'room_scope_read', from_level: 'trusted', enforced: true },
    { approval: 'room_scope_plan', from_level: 'trusted', enforced: true },
    { approval: 'validations_docker', from_level: 'trusted', enforced: true },
    { approval: 'code_write_other_project', from_level: 'full', enforced: false },
    { approval: 'validations_unconfined', from_level: 'full', enforced: true },
    { approval: 'host_access', from_level: 'full', enforced: false },
  ],
  never_covered: ['credentials', 'trust_settings', 'destructive_confirmations', 'peer_inheritance'],
  sandbox: { available: true, protect_vault: true, protected: true },
  proof: { enrolled: true, pending: false, recovery_codes_left: 10, locked_for_secs: 0 },
};

function view(level: TrustLevel, source: TrustScope = 'instance', own: TrustLevel | null = level): TrustScopeView {
  const effective: EffectiveTrust = { level, source, explicit: true };
  return {
    setting: { scope: source, level: own, set_by: 'Romu', set_at: '2026-10-28T00:00:00Z' } as TrustScopeView['setting'],
    effective,
  };
}

const wrap = (node: React.ReactNode) => render(<I18nProvider>{node}</I18nProvider>);

beforeEach(() => {
  Object.values(trustMock).forEach(mock => mock.mockReset());
  trustMock.overview.mockResolvedValue(OVERVIEW);
  trustMock.audit.mockResolvedValue([]);
});
afterEach(cleanup);

describe('TrustLevelControl', () => {
  it('opens the risk panel for Trusted and lists exactly what Trusted changes', async () => {
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    const panel = await screen.findByTestId('trust-risk-panel');

    const approvals = within(panel).getAllByRole('listitem')
      .map(item => item.getAttribute('data-approval'))
      .filter(Boolean);
    expect(approvals).toEqual([
      'quick_exec_line', 'exec_step_line', 'room_scope_read', 'room_scope_plan', 'validations_docker',
    ]);
    expect(trustMock.setInstance).not.toHaveBeenCalled();
  });

  it('lists the Full access effects too, flags the ones not enforced yet, and names what is never covered', async () => {
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-full'));
    const panel = await screen.findByTestId('trust-risk-panel');

    for (const approval of ['code_write_other_project', 'validations_unconfined', 'host_access']) {
      expect(panel.querySelector(`[data-approval="${approval}"]`)).not.toBeNull();
    }
    expect(panel.querySelector('[data-approval="code_write_other_project"] .trust-pending-tag')).not.toBeNull();
    expect(panel.querySelector('[data-approval="validations_unconfined"] .trust-pending-tag')).toBeNull();
    expect(within(screen.getByTestId('trust-never')).getAllByRole('listitem')).toHaveLength(4);
  });

  it('needs the explicit confirmation and a code from the authenticator before anything is saved', async () => {
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    trustMock.setInstance.mockResolvedValue(view('trusted'));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    const confirm = await screen.findByTestId('trust-confirm');
    expect(confirm).toBeDisabled();
    fireEvent.click(confirm);
    expect(trustMock.setInstance).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('trust-ack'));
    expect(confirm, 'the acknowledgement alone is not enough').toBeDisabled();
    fireEvent.change(screen.getByTestId('trust-proof-input'), { target: { value: ' 123456 ' } });
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    await waitFor(() => expect(trustMock.setInstance).toHaveBeenCalledWith({
      level: 'trusted', acknowledged_risks: true, acknowledged_vault_unprotected: false, proof: '123456',
    }));
    await waitFor(() => expect(screen.queryByTestId('trust-risk-panel')).toBeNull());
    // The vault is protected here: no extra acknowledgement, no warning.
    expect(screen.queryByTestId('trust-vault-note')).toBeNull();
  });

  it('keeps the panel open and saves nothing when the server refuses the code', async () => {
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    trustMock.setInstance.mockRejectedValue(new Error('the confirmation code is wrong'));
    const toast = vi.fn();
    wrap(<TrustLevelControl target={{ scope: 'instance' }} toast={toast} />);

    fireEvent.click(await screen.findByTestId('trust-level-full'));
    await screen.findByTestId('trust-risk-panel');
    fireEvent.click(screen.getByTestId('trust-ack'));
    fireEvent.change(screen.getByTestId('trust-proof-input'), { target: { value: '000000' } });
    fireEvent.click(screen.getByTestId('trust-confirm'));

    await waitFor(() => expect(toast).toHaveBeenCalled());
    expect(screen.getByTestId('trust-risk-panel')).toBeTruthy();
    expect(trustMock.setInstance).toHaveBeenCalledTimes(1);
  });

  it('says what stays reachable and what stays closed, and no longer claims secrets are never readable', async () => {
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-full'));
    await screen.findByTestId('trust-risk-panel');

    const reachable = within(screen.getByTestId('trust-reachable')).getAllByRole('listitem');
    const reachableText = reachable.map(item => item.textContent).join(' ');
    expect(reachableText).toMatch(/Docker/);
    // The network and Kronn's own API stay open: the MCP bridge goes through it.
    expect(reachableText).toMatch(/réseau/);
    expect(reachableText).toMatch(/API de Kronn/);
    expect(reachable).toHaveLength(6);
    // Only Kronn's data directory is closed.
    const closed = within(screen.getByTestId('trust-closed')).getAllByRole('listitem');
    expect(closed).toHaveLength(1);
    expect(closed[0].textContent).toMatch(/clé de chiffrement/);
    expect(screen.getByTestId('trust-closed').textContent).not.toMatch(/port d'API|réseau est fermé/);
    expect(screen.getByTestId('trust-never').textContent)
      .not.toMatch(/jamais lisibles|never readable|nunca son legibles/i);
    // The panel states the limit: the code protects against what a level
    // approves; the agent processes themselves are confined by a separate protection
    // of the same release (0.15.2), named in words, never by a ticket number.
    const limit = screen.getByTestId('trust-limit').textContent ?? '';
    expect(limit).not.toMatch(/KT-\d+/);
    expect(limit).toMatch(/protection distincte/);
    expect(limit).toMatch(/0\.15\.2/);
  });

  it('blocks the choice, and points to the enrolment, when no authenticator is set up', async () => {
    trustMock.overview.mockResolvedValue({
      ...OVERVIEW,
      proof: { enrolled: false, pending: false, recovery_codes_left: 0, locked_for_secs: 0 },
    });
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    await screen.findByTestId('trust-proof-missing');
    expect(screen.getByTestId('trust-proof-enrol-link').getAttribute('href')).toContain('settings-trust');
    expect(screen.queryByTestId('trust-proof-input')).toBeNull();
    fireEvent.click(screen.getByTestId('trust-ack'));
    expect(screen.getByTestId('trust-confirm')).toBeDisabled();
  });

  it('does not block a host without a sandbox: it names the missing vault protection and asks for its own acknowledgement', async () => {
    trustMock.overview.mockResolvedValue({
      ...OVERVIEW,
      sandbox: {
        available: false,
        reason: 'bwrap is not installed',
        protect_vault: true,
        protected: false,
        missing: 'Kronn vault protection is unavailable on this machine (no sandbox-exec or bubblewrap)',
      },
    });
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    trustMock.setInstance.mockResolvedValue(view('full'));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    // The level control itself states it, before any panel opens.
    const note = await screen.findByTestId('trust-vault-note');
    expect(note.textContent).toMatch(/protection du coffre Kronn est indisponible sur cette machine/);
    expect(note.textContent).toMatch(/sandbox-exec ni bubblewrap/);

    fireEvent.click(await screen.findByTestId('trust-level-full'));
    const gap = await screen.findByTestId('trust-vault-unprotected');
    expect(gap.textContent).toContain('bwrap is not installed');
    expect(gap.textContent).toMatch(/indisponible sur cette machine/);

    // The proof and the risk acknowledgement are not enough: the missing
    // protection has its own acknowledgement.
    const confirm = screen.getByTestId('trust-confirm');
    fireEvent.click(screen.getByTestId('trust-ack'));
    fireEvent.change(screen.getByTestId('trust-proof-input'), { target: { value: '123456' } });
    expect(confirm).toBeDisabled();
    fireEvent.click(screen.getByTestId('trust-vault-ack'));
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    await waitFor(() => expect(trustMock.setInstance).toHaveBeenCalledWith({
      level: 'full', acknowledged_risks: true, acknowledged_vault_unprotected: true, proof: '123456',
    }));
  });

  it('names the setting when the owner turned the vault protection off', async () => {
    trustMock.overview.mockResolvedValue({
      ...OVERVIEW,
      sandbox: { available: true, protect_vault: false, protected: false },
    });
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    expect((await screen.findByTestId('trust-vault-note')).textContent).toMatch(/protect_vault_from_agents/);
    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    expect((await screen.findByTestId('trust-vault-unprotected')).textContent)
      .toMatch(/protect_vault_from_agents = false/);
    expect(screen.getByTestId('trust-vault-ack')).toBeTruthy();
  });

  it('says how long a locked confirmation stays locked, and still takes a recovery code', async () => {
    trustMock.overview.mockResolvedValue({
      ...OVERVIEW,
      proof: { enrolled: true, pending: false, recovery_codes_left: 10, locked_for_secs: 42 },
    });
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    trustMock.setInstance.mockResolvedValue(view('trusted'));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    expect((await screen.findByTestId('trust-proof-locked')).textContent).toContain('42');
    // The server accepts a recovery code during a lockout: the owner can type one.
    const input = screen.getByTestId('trust-proof-input');
    fireEvent.change(input, { target: { value: 'abcd-efgh-ijkl' } });
    fireEvent.click(screen.getByTestId('trust-ack'));
    const confirm = screen.getByTestId('trust-confirm');
    expect(confirm).toBeEnabled();
    fireEvent.click(confirm);
    await waitFor(() => expect(trustMock.setInstance).toHaveBeenCalledWith({
      level: 'trusted', acknowledged_risks: true, acknowledged_vault_unprotected: false, proof: 'abcd-efgh-ijkl',
    }));
  });

  it('re-reads the lock after a refused code and when the lockout ends', async () => {
    const locked = {
      ...OVERVIEW,
      proof: { enrolled: true, pending: false, recovery_codes_left: 10, locked_for_secs: 1 },
    };
    trustMock.overview.mockResolvedValueOnce(OVERVIEW).mockResolvedValueOnce(locked).mockResolvedValue(OVERVIEW);
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    trustMock.setInstance.mockRejectedValue(new Error('wrong code'));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    fireEvent.change(await screen.findByTestId('trust-proof-input'), { target: { value: '000000' } });
    fireEvent.click(screen.getByTestId('trust-ack'));
    fireEvent.click(screen.getByTestId('trust-confirm'));
    // The refusal started a lockout: the panel shows it...
    await screen.findByTestId('trust-proof-locked');
    // ...and drops it by itself once it is over.
    await waitFor(() => expect(screen.queryByTestId('trust-proof-locked')).toBeNull(), { timeout: 3000 });
  });

  it('lowers from Full to Trusted without a panel and without a code', async () => {
    trustMock.instance.mockResolvedValue(view('full'));
    trustMock.setInstance.mockResolvedValue(view('trusted'));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    await waitFor(() => expect(trustMock.setInstance).toHaveBeenCalledWith({
      level: 'trusted', acknowledged_risks: false, acknowledged_vault_unprotected: false,
    }));
    expect(screen.queryByTestId('trust-risk-panel')).toBeNull();
  });

  it('saves nothing when the panel is cancelled', async () => {
    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-full'));
    await screen.findByTestId('trust-risk-panel');
    fireEvent.click(screen.getByTestId('trust-cancel'));
    expect(screen.queryByTestId('trust-risk-panel')).toBeNull();
    expect(trustMock.setInstance).not.toHaveBeenCalled();
  });

  it('lowers to Standard without a panel', async () => {
    trustMock.instance.mockResolvedValue(view('full'));
    trustMock.setInstance.mockResolvedValue(view('standard'));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);

    fireEvent.click(await screen.findByTestId('trust-level-standard'));
    await waitFor(() => expect(trustMock.setInstance).toHaveBeenCalledWith({
      level: 'standard', acknowledged_risks: false, acknowledged_vault_unprotected: false,
    }));
    expect(screen.queryByTestId('trust-risk-panel')).toBeNull();
  });

  it('shows the link to Settings › Trust levels from a room or a workflow panel, not from the settings itself', async () => {
    trustMock.get.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'room', id: 'r1' }} />);
    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    const link = await screen.findByTestId('trust-panel-settings-link');
    expect(link.getAttribute('href')).toContain('settings-trust');
    cleanup();

    trustMock.instance.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'instance' }} />);
    fireEvent.click(await screen.findByTestId('trust-level-trusted'));
    await screen.findByTestId('trust-risk-panel');
    expect(screen.queryByTestId('trust-panel-settings-link')).toBeNull();
  });

  it('saves a room level against the room and can follow the wider setting again', async () => {
    trustMock.get.mockResolvedValue(view('trusted', 'room'));
    trustMock.clear.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustLevelControl target={{ scope: 'room', id: 'r1' }} />);

    fireEvent.click(await screen.findByTestId('trust-inherit'));
    await waitFor(() => expect(trustMock.clear).toHaveBeenCalledWith('room', 'r1'));
  });
});

describe('TrustBadge', () => {
  it('shows nothing under Standard', async () => {
    trustMock.get.mockResolvedValue(view('standard', 'instance', null));
    wrap(<TrustBadge target={{ scope: 'room', id: 'r1' }} />);
    await waitFor(() => expect(trustMock.get).toHaveBeenCalled());
    expect(screen.queryByTestId('trust-badge-room')).toBeNull();
  });

  it('shows the level in force and where it comes from, and revokes in one click', async () => {
    trustMock.get.mockResolvedValue(view('full', 'instance', null));
    trustMock.set.mockResolvedValue(view('standard', 'room'));
    wrap(<TrustBadge target={{ scope: 'room', id: 'r1' }} />);

    const badge = await screen.findByTestId('trust-badge-room');
    expect(badge.getAttribute('data-level')).toBe('full');
    expect(badge.textContent).toMatch(/Accès complet.*instance/);

    fireEvent.click(screen.getByTestId('trust-revoke'));
    await waitFor(() => expect(trustMock.set).toHaveBeenCalledWith('room', 'r1', {
      level: 'standard', acknowledged_risks: false, acknowledged_vault_unprotected: false,
    }));
  });

  it('shows nothing, rather than breaking its host, when the level cannot be read', async () => {
    trustMock.get.mockRejectedValue(new Error('down'));
    wrap(<TrustBadge target={{ scope: 'workflow', id: 'w1' }} />);
    await waitFor(() => expect(trustMock.get).toHaveBeenCalled());
    expect(screen.queryByTestId('trust-badge-workflow')).toBeNull();
  });
});

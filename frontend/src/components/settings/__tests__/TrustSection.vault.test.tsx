/**
 * KT-1495 — Settings › Security says whether Kronn's vault is closed to the
 * agent processes: active, turned off in the config, or unavailable on this
 * machine. Nothing is refused because of the last two.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { cleanup, render, screen } from '@testing-library/react';
import type { TrustOverview } from '../../../types/generated';

const { trustMock } = vi.hoisted(() => ({
  trustMock: {
    overview: vi.fn(),
    audit: vi.fn(),
    instance: vi.fn(),
    proof: vi.fn(),
  },
}));
vi.mock('../../../lib/api', async () => {
  const real = await vi.importActual<Record<string, object>>('../../../lib/api');
  return { ...real, trust: trustMock };
});

import { I18nProvider } from '../../../lib/I18nContext';
import { TrustSection } from '../TrustSection';

const OVERVIEW: TrustOverview = {
  instance: { scope: 'instance', level: 'standard' },
  effects: [],
  never_covered: [],
  sandbox: { available: true, protect_vault: true, protected: true },
  proof: { enrolled: false, pending: false, recovery_codes_left: 0, locked_for_secs: 0 },
};

const withSandbox = (sandbox: TrustOverview['sandbox']): TrustOverview => ({ ...OVERVIEW, sandbox });

beforeEach(() => {
  Object.values(trustMock).forEach(mock => mock.mockReset());
  trustMock.audit.mockResolvedValue([]);
  trustMock.instance.mockResolvedValue({
    setting: { scope: 'instance', level: null },
    effective: { level: 'standard', source: 'default', explicit: false },
  });
});
afterEach(cleanup);

const render_ = () => render(<I18nProvider><TrustSection /></I18nProvider>);

describe('TrustSection vault status', () => {
  it('says the vault is closed to the agents when the sandbox is active', async () => {
    trustMock.overview.mockResolvedValue(OVERVIEW);
    render_();
    const status = await screen.findByTestId('trust-vault-status');
    expect(status.getAttribute('data-state')).toBe('active');
  });

  it('reports the protection as unavailable, with the reason', async () => {
    trustMock.overview.mockResolvedValue(withSandbox({
      available: false, reason: '`bwrap` is not installed', protect_vault: true, protected: false,
      missing: 'Kronn vault protection is unavailable on this machine',
    }));
    render_();
    const status = await screen.findByTestId('trust-vault-status');
    expect(status.getAttribute('data-state')).toBe('unavailable');
    expect(status.textContent).toContain('`bwrap` is not installed');
  });

  it('reports the protection as turned off when the owner chose so', async () => {
    trustMock.overview.mockResolvedValue(withSandbox({
      available: true, protect_vault: false, protected: false, missing: 'off',
    }));
    render_();
    const status = await screen.findByTestId('trust-vault-status');
    expect(status.getAttribute('data-state')).toBe('off');
    expect(status.textContent).toContain('protect_vault_from_agents');
  });
});

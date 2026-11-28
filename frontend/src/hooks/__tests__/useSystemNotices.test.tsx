/** KT-1686 — what the install has to say goes to the bell, never above the menu. */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, waitFor, act } from '@testing-library/react';

const { config, version, mcps } = vi.hoisted(() => ({
  config: { getRecoveryStatus: vi.fn() },
  version: { check: vi.fn() },
  mcps: { overview: vi.fn() },
}));
vi.mock('../../lib/api', () => ({ config, version, mcps }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: (string | number)[]) => (args.length ? `${key} ${args.join(' ')}` : key) }),
}));

import { PROTECT_SNOOZE_KEY, PROTECT_SNOOZE_MS, UPDATE_DISMISS_KEY, useSystemNotices } from '../useSystemNotices';
import { dismissNotice, getNotifications, resetNotifications } from '../../lib/notifications';

function Host({ resetError = null, onDismiss = () => {} }: { resetError?: string | null; onDismiss?: () => void }) {
  useSystemNotices(resetError, onDismiss);
  return null;
}

const kinds = () => getNotifications().notices.map(n => n.kind);
const settle = async () => { await act(async () => {}); await act(async () => {}); };

describe('useSystemNotices', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetNotifications();
    localStorage.clear();
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false });
    version.check.mockResolvedValue({ current: '0.15.3', latest: null, up_to_date: true, release_url: null });
    mcps.overview.mockResolvedValue({ configs: [] });
  });
  afterEach(() => cleanup());

  it('reports a locked key with the credentials still in clear and the place that restores it', async () => {
    config.getRecoveryStatus.mockResolvedValue({
      key_locked: true,
      config_holds_key: true,
      clear_credentials: [{ kind: 'provider_key', name: 'Anthropic', provider: 'anthropic', verified: false }],
    });
    render(<Host />);
    await waitFor(() => expect(kinds()).toEqual(['key_locked']));
    const [notice] = getNotifications().notices;
    expect(notice.link).toBe('/config/security#settings-recovery');
    expect(notice.args?.[0]).toContain('Anthropic');
    expect(notice.dismiss).toBeUndefined();
  });

  it('reports a key beside its data, unreadable credentials and a config set aside', async () => {
    config.getRecoveryStatus.mockResolvedValue({
      key_locked: false, config_holds_key: true, credentials_unavailable: 'disk full', config_set_aside: 'kept as config.toml.corrupt.1',
    });
    render(<Host />);
    await waitFor(() => expect(kinds()).toEqual(['key_beside_data', 'credentials_unavailable', 'config_set_aside']));
    const [, credentials, aside] = getNotifications().notices;
    expect(credentials.args).toEqual(['disk full']);
    expect(aside.args).toEqual(['kept as config.toml.corrupt.1']);
    dismissNotice(aside.id);
    expect(kinds()).toEqual(['key_beside_data', 'credentials_unavailable']);
  });

  it('puts the advice to protect the secrets away for a week on "Later", then shows it again', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, config_holds_key: true, configured: false });
    render(<Host />);
    await waitFor(() => expect(kinds()).toEqual(['key_beside_data']));
    const before = Date.now();
    dismissNotice(getNotifications().notices[0].id);
    const until = Number(localStorage.getItem(PROTECT_SNOOZE_KEY));
    expect(until).toBeGreaterThanOrEqual(before + PROTECT_SNOOZE_MS);
    expect(kinds()).toEqual([]);

    cleanup();
    render(<Host />);
    await waitFor(() => expect(config.getRecoveryStatus).toHaveBeenCalledTimes(2));
    await settle();
    expect(kinds()).toEqual([]);

    cleanup();
    localStorage.setItem(PROTECT_SNOOZE_KEY, String(Date.now() - 1));
    render(<Host />);
    await waitFor(() => expect(kinds()).toEqual(['key_beside_data']));
  });

  it('does not repeat the advice once a recovery phrase protects the key in use', async () => {
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, config_holds_key: true, configured: true, matches_key: true });
    render(<Host />);
    await waitFor(() => expect(config.getRecoveryStatus).toHaveBeenCalled());
    await settle();
    expect(kinds()).toEqual([]);

    cleanup();
    config.getRecoveryStatus.mockResolvedValue({ key_locked: false, config_holds_key: true, configured: true, matches_key: false });
    render(<Host />);
    await waitFor(() => expect(kinds()).toEqual(['key_beside_data']));
  });

  it('reports nothing for a healthy install', async () => {
    render(<Host />);
    await waitFor(() => expect(version.check).toHaveBeenCalled());
    await settle();
    expect(kinds()).toEqual([]);
  });

  it('reports an update once per version: put away, it stays away until the next release', async () => {
    version.check.mockResolvedValue({ current: '0.15.2', latest: '0.15.3', up_to_date: false, release_url: 'https://example.test/r' });
    render(<Host />);
    await waitFor(() => expect(kinds()).toEqual(['update_available']));
    const [notice] = getNotifications().notices;
    expect(notice).toMatchObject({ args: ['0.15.2', '0.15.3'], link: 'https://example.test/r', external: true });
    dismissNotice(notice.id);
    expect(localStorage.getItem(UPDATE_DISMISS_KEY)).toBe('0.15.3');
    expect(kinds()).toEqual([]);

    cleanup();
    render(<Host />);
    await waitFor(() => expect(version.check).toHaveBeenCalledTimes(2));
    await settle();
    expect(kinds()).toEqual([]);

    cleanup();
    version.check.mockResolvedValue({ current: '0.15.2', latest: '0.15.4', up_to_date: false, release_url: null });
    render(<Host />);
    await waitFor(() => expect(kinds()).toEqual(['update_available']));
  });

  it('reports a failed reset with the backend message, and clears it where it came from', async () => {
    const onDismiss = vi.fn();
    const view = render(<Host resetError="disk full" onDismiss={onDismiss} />);
    await waitFor(() => expect(kinds()).toEqual(['reset_error']));
    expect(getNotifications().notices[0].args).toEqual(['disk full']);
    dismissNotice(getNotifications().notices[0].id);
    expect(onDismiss).toHaveBeenCalledTimes(1);
    view.rerender(<Host resetError={null} onDismiss={onDismiss} />);
    expect(kinds()).toEqual([]);
  });
});

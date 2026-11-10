/**
 * 0.8.7 — P1-6 of the QA roadmap.
 *
 * IdentitySection ships pseudo / avatar email / bio / global-context to
 * the persisted server config. Pre-test it had ZERO coverage, so a
 * regression in the save-on-change behaviour (e.g. dropped `pseudo`
 * field in the payload) would silently lose the user's identity at
 * every page-load. Pins :
 *  - mount loads the existing config + global context + mode
 *  - typing in pseudo / email / bio fires `configApi.setServerConfig`
 *    with the right partial payload
 *  - gravatar preview appears once the email looks like an email
 *  - global-context save is deferred to `onBlur` and only fires when
 *    the field is dirty
 *  - global-context mode change persists immediately via `saveGlobalContextMode`
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, act, cleanup, waitFor } from '@testing-library/react';

const { config, contacts, setAuthToken } = vi.hoisted(() => ({
  config: {
    getServerConfig: vi.fn(),
    setServerConfig: vi.fn(),
    getGlobalContext: vi.fn(),
    saveGlobalContext: vi.fn(),
    getGlobalContextMode: vi.fn(),
    saveGlobalContextMode: vi.fn(),
    getNetworkExposure: vi.fn(),
    setNetworkExposure: vi.fn(),
    getDesktopPort: vi.fn(),
  },
  contacts: {
    networkInfo: vi.fn(),
    inviteCode: vi.fn(),
  },
  setAuthToken: vi.fn(),
}));

vi.mock('../../../lib/api', () => ({ config, contacts, setAuthToken }));

const { flushUiPreferences } = vi.hoisted(() => ({ flushUiPreferences: vi.fn() }));
vi.mock('../../../lib/uiPreferences', () => ({ flushUiPreferences }));

import { IdentitySection } from '../IdentitySection';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}(${args.join('|')})` : key;

beforeEach(() => {
  // Reasonable defaults — tests override per-case.
  config.getServerConfig.mockResolvedValue({
    domain: null, port: 3140, max_concurrent_agents: 5, auth_enabled: true,
    pseudo: '', avatar_email: '', bio: '',
  });
  config.setServerConfig.mockResolvedValue(undefined);
  config.getGlobalContext.mockResolvedValue('');
  config.saveGlobalContext.mockResolvedValue(undefined);
  config.getGlobalContextMode.mockResolvedValue('always');
  config.saveGlobalContextMode.mockResolvedValue(undefined);
  config.getNetworkExposure.mockResolvedValue({ exposed: false, restart_required: false, port: 3140, reachable_ips: [], auth_token_set: false });
  config.getDesktopPort.mockResolvedValue({ saved: 47315, current: 47315, min: 1024, max: 65535 });
  config.setNetworkExposure.mockResolvedValue({ exposed: true, restart_required: true, port: 3140, reachable_ips: [], auth_token_set: true });
  contacts.networkInfo.mockResolvedValue({
    tailscale_ip: null, advertised_host: null, detected_ips: [],
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function mountIdentity(toast = vi.fn(), onExposureChange?: (exposure: unknown) => void) {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<IdentitySection toast={toast as never} t={t} onExposureChange={onExposureChange} />);
  });
  // Let the mount effects settle (getServerConfig / getGlobalContext /
  // getGlobalContextMode / networkInfo all resolve in microtasks).
  await act(async () => { await new Promise(r => setTimeout(r, 0)); });
  return result!;
}

describe('IdentitySection — mount load', () => {
  it('hydrates pseudo / avatar_email / bio from the server config', async () => {
    config.getServerConfig.mockResolvedValue({
      domain: 'kronn.local', port: 3140, max_concurrent_agents: 5, auth_enabled: true,
      pseudo: 'JohnDoe42', avatar_email: 'john@example.com', bio: 'Tester',
    });
    await mountIdentity();
    expect((screen.getByPlaceholderText('settings.pseudoPlaceholder') as HTMLInputElement).value).toBe('JohnDoe42');
    expect((screen.getByPlaceholderText('settings.avatarEmailPlaceholder') as HTMLInputElement).value).toBe('john@example.com');
    // bio uses the i18n placeholder key (identity translator → verbatim).
    expect((screen.getByPlaceholderText('settings.bioPlaceholder') as HTMLTextAreaElement).value).toBe('Tester');
  });

  it('loads global context body + mode on mount', async () => {
    config.getGlobalContext.mockResolvedValue('# My notes\n- foo');
    config.getGlobalContextMode.mockResolvedValue('no_project');
    await mountIdentity();
    expect(config.getGlobalContext).toHaveBeenCalled();
    expect(config.getGlobalContextMode).toHaveBeenCalled();
    expect((screen.getByPlaceholderText('settings.globalContextPlaceholder') as HTMLTextAreaElement).value)
      .toBe('# My notes\n- foo');
  });

  it('survives API errors on mount without crashing', async () => {
    // Defensive : every fetch is `.catch(() => {})`. Test that a
    // rejected getServerConfig leaves the card mounted with empty fields.
    config.getServerConfig.mockRejectedValue(new Error('500'));
    config.getGlobalContext.mockRejectedValue(new Error('500'));
    config.getGlobalContextMode.mockRejectedValue(new Error('500'));
    contacts.networkInfo.mockRejectedValue(new Error('500'));
    await mountIdentity();
    // Card mounted ; pseudo input is empty.
    expect((screen.getByPlaceholderText('settings.pseudoPlaceholder') as HTMLInputElement).value).toBe('');
  });

  it('separates profile, personal instructions and network sharing', async () => {
    await mountIdentity();
    expect(screen.getByText('settings.identityProfileTitle')).toBeInTheDocument();
    expect(screen.getByText('settings.identityContextTitle')).toBeInTheDocument();
    expect(screen.getByText('settings.identityNetworkTitle')).toBeInTheDocument();
    expect(document.querySelectorAll('.set-identity-panel')).toHaveLength(3);
  });

  it('keeps the connection guide collapsed until requested', async () => {
    await mountIdentity();
    expect(screen.queryByText('contacts.guideStep1')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /contacts\.guideTitle/ }));
    expect(screen.getByText('contacts.guideStep1')).toBeInTheDocument();
  });

  it('distinguishes discussion-launched agents from full CLI usage', async () => {
    await mountIdentity();
    fireEvent.click(screen.getByRole('button', { name: 'settings.bioInfoTitle' }));

    expect(screen.getByText('settings.bioInfoInjection')).toBeInTheDocument();
    expect(screen.getByText('settings.bioInfoCli')).toBeInTheDocument();
  });

  it('explains the exact scope of global context injection', async () => {
    await mountIdentity();
    fireEvent.click(screen.getByRole('button', { name: 'settings.globalContextInfoTitle' }));

    expect(screen.getByText('settings.globalContextInfoInjection')).toBeInTheDocument();
    expect(screen.getByText('settings.globalContextInfoCli')).toBeInTheDocument();
  });
});

describe('IdentitySection — save-on-change', () => {
  it('typing in pseudo fires setServerConfig with a partial payload', async () => {
    await mountIdentity();
    const input = screen.getByPlaceholderText('settings.pseudoPlaceholder') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'Romuald' } });
    expect(config.setServerConfig).toHaveBeenCalledWith({ pseudo: 'Romuald' });
    expect(input.value).toBe('Romuald');
  });

  it('typing in avatar email fires setServerConfig with avatar_email only', async () => {
    await mountIdentity();
    const input = screen.getByPlaceholderText('settings.avatarEmailPlaceholder') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'me@example.com' } });
    expect(config.setServerConfig).toHaveBeenCalledWith({ avatar_email: 'me@example.com' });
  });

  it('typing in bio fires setServerConfig with bio only', async () => {
    await mountIdentity();
    const ta = screen.getByPlaceholderText('settings.bioPlaceholder') as HTMLTextAreaElement;
    fireEvent.change(ta, { target: { value: 'Eng @ Euronews' } });
    expect(config.setServerConfig).toHaveBeenCalledWith({ bio: 'Eng @ Euronews' });
  });

  it('surfaces an identity save failure without discarding the typed value', async () => {
    const toast = vi.fn();
    config.setServerConfig.mockRejectedValueOnce(new Error('offline'));
    await mountIdentity(toast);
    const input = screen.getByPlaceholderText('settings.pseudoPlaceholder') as HTMLInputElement;

    fireEvent.change(input, { target: { value: 'Romuald' } });

    expect(input.value).toBe('Romuald');
    await waitFor(() => expect(toast).toHaveBeenCalledWith(
      expect.stringContaining('offline'),
      'error',
    ));
  });
});

describe('IdentitySection — gravatar preview', () => {
  it('does NOT render the gravatar img when the email is empty', async () => {
    await mountIdentity();
    expect(document.querySelector('img.set-gravatar-img')).toBeNull();
  });

  it('does NOT render the gravatar img when the email lacks "@"', async () => {
    config.getServerConfig.mockResolvedValue({
      domain: null, port: 3140, max_concurrent_agents: 5, auth_enabled: true,
      pseudo: '', avatar_email: 'not-an-email', bio: '',
    });
    await mountIdentity();
    expect(document.querySelector('img.set-gravatar-img')).toBeNull();
  });

  it('renders the gravatar img once the email looks like an email', async () => {
    config.getServerConfig.mockResolvedValue({
      domain: null, port: 3140, max_concurrent_agents: 5, auth_enabled: true,
      pseudo: '', avatar_email: 'me@example.com', bio: '',
    });
    await mountIdentity();
    const img = document.querySelector('img.set-gravatar-img') as HTMLImageElement | null;
    expect(img).not.toBeNull();
    expect(img!.src).toMatch(/gravatar\.com/);
  });
});

describe('IdentitySection — global context lifecycle', () => {
  it('global context body saves on blur ONLY when dirty (no save on initial blur)', async () => {
    await mountIdentity();
    const ta = screen.getByPlaceholderText('settings.globalContextPlaceholder') as HTMLTextAreaElement;
    fireEvent.blur(ta);
    expect(config.saveGlobalContext).not.toHaveBeenCalled();

    // Now mark dirty by typing.
    fireEvent.change(ta, { target: { value: '## Updated' } });
    expect(config.saveGlobalContext).not.toHaveBeenCalled(); // still deferred to blur
    expect(screen.getByRole('status')).toHaveTextContent('settings.globalContextPending');

    fireEvent.blur(ta);
    await waitFor(() => expect(config.saveGlobalContext).toHaveBeenCalledWith('## Updated'));
  });

  it('global context mode change persists immediately via saveGlobalContextMode', async () => {
    await mountIdentity();
    // Dropdown exposes a stable testId. Open via click then pick option.
    const trigger = screen.getByTestId('settings-global-context-mode');
    fireEvent.click(trigger);
    // The option labels go through the identity translator, so option
    // texts equal the i18n key (e.g. 'settings.gcModeNever').
    const neverOption = await screen.findByText('settings.gcModeNever');
    fireEvent.click(neverOption);
    await waitFor(() => expect(config.saveGlobalContextMode).toHaveBeenCalledWith('never'));
  });
});

describe('IdentitySection — plain-HTTP exposure (KT-1596)', () => {
  const declared = { exposed: false, restart_required: false, in_docker: false, kronn_bind: null, port: 3140, reachable_ips: [], auth_token_set: true, tls_terminated_by_proxy: false, tls_declared_by_env: false, public_origin: null, plain_http_blocked: false };

  it('shows the backend refusal next to the toggle, which stays off', async () => {
    config.getNetworkExposure.mockResolvedValue(declared);
    config.setNetworkExposure.mockRejectedValue(new Error('Kronn cannot be exposed over plain HTTP: declare a TLS proxy first.'));
    await mountIdentity();
    const toggle = screen.getByTestId('expose-network-toggle');

    await act(async () => { fireEvent.click(toggle); });

    expect(await screen.findByTestId('expose-refusal')).toHaveTextContent('Kronn cannot be exposed over plain HTTP');
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    expect(screen.queryByText('settings.exposeRestartRequired')).not.toBeInTheDocument();
  });

  it('explains why a stale exposed config started on loopback', async () => {
    config.getNetworkExposure.mockResolvedValue({ ...declared, exposed: true, plain_http_blocked: true });
    await mountIdentity();
    expect(screen.getByTestId('expose-plain-http-blocked')).toHaveTextContent('settings.exposePlainBlocked');
  });

  it('offers no plain invite and one consistent notice while exposure is blocked', async () => {
    config.getServerConfig.mockResolvedValue({
      domain: null, port: 3140, max_concurrent_agents: 5, auth_enabled: true,
      pseudo: 'me', avatar_email: '', bio: '',
    });
    config.getNetworkExposure.mockResolvedValue({ ...declared, exposed: true, plain_http_blocked: true });
    await mountIdentity();

    // One notice about the block: no "reachable from your network" note beside it,
    // and the state badge agrees (this boot serves nobody).
    expect(screen.getByTestId('expose-plain-http-blocked')).toBeInTheDocument();
    expect(screen.queryByText('settings.exposeSecurityNote')).not.toBeInTheDocument();
    expect(screen.getByText('settings.networkLocalOnly')).toBeInTheDocument();
    expect(screen.queryByText('settings.networkReachable')).not.toBeInTheDocument();
    // No plain kronn: invite is advertised, and no copy button for one.
    expect(screen.getByTestId('invite-blocked')).toHaveTextContent('settings.identityInviteBlocked');
    expect(screen.queryByText(/^kronn:me@/)).not.toBeInTheDocument();
  });

  it('keeps the invite and the exposure note when nothing is blocked', async () => {
    config.getServerConfig.mockResolvedValue({
      domain: null, port: 3140, max_concurrent_agents: 5, auth_enabled: true,
      pseudo: 'me', avatar_email: '', bio: '',
    });
    config.getNetworkExposure.mockResolvedValue({ ...declared, exposed: true, tls_terminated_by_proxy: true, public_origin: 'https://kronn.example.com' });
    contacts.inviteCode.mockResolvedValue('kronns:me@kronn.example.com:443');
    await mountIdentity();
    expect(screen.queryByTestId('invite-blocked')).not.toBeInTheDocument();
    expect(await screen.findByText('kronns:me@kronn.example.com:443')).toBeInTheDocument();
    expect(screen.getByText('settings.exposeSecurityNote')).toBeInTheDocument();
    expect(screen.getByText('settings.networkReachable')).toBeInTheDocument();
  });

  it('says nothing about plain HTTP once a proxy is declared', async () => {
    config.getNetworkExposure.mockResolvedValue({ ...declared, exposed: true, tls_terminated_by_proxy: true, public_origin: 'https://kronn.example.com' });
    contacts.inviteCode.mockResolvedValue('kronns:me@kronn.example.com:443');
    await mountIdentity();
    expect(screen.queryByTestId('expose-plain-http-blocked')).not.toBeInTheDocument();
    expect(screen.getByTestId('expose-proxy-origin')).toHaveValue('https://kronn.example.com');
  });

  it('declares the proxy with its public address and keeps the token the reply hands over', async () => {
    config.getNetworkExposure.mockResolvedValue(declared);
    config.setNetworkExposure.mockResolvedValue({ ...declared, tls_terminated_by_proxy: true, public_origin: 'https://kronn.example.com', issued_auth_token: 'tok-123' });
    const onExposureChange = vi.fn();
    await mountIdentity(vi.fn(), onExposureChange);
    const save = screen.getByTestId('expose-proxy-save');
    expect(save).toBeDisabled();

    fireEvent.change(screen.getByTestId('expose-proxy-origin'), { target: { value: ' https://kronn.example.com ' } });
    await act(async () => { fireEvent.click(save); });

    await waitFor(() => expect(config.setNetworkExposure).toHaveBeenCalledWith(false, {
      tls_terminated_by_proxy: true,
      public_origin: 'https://kronn.example.com',
    }));
    // The loopback bypass is withdrawn: the local UI must hold the token now.
    expect(setAuthToken).toHaveBeenCalledWith('tok-123');
    // The token never lingers in the state handed to the rest of the page.
    expect(onExposureChange).toHaveBeenCalledWith(expect.not.objectContaining({ issued_auth_token: 'tok-123' }));
    expect(await screen.findByTestId('expose-proxy-withdraw')).toBeInTheDocument();
  });

  it('shows the refusal of a bad declaration and withdraws a declaration with the exposure', async () => {
    config.getNetworkExposure.mockResolvedValue({ ...declared, exposed: true, tls_terminated_by_proxy: true, public_origin: 'https://kronn.example.com' });
    contacts.inviteCode.mockResolvedValue('kronns:me@kronn.example.com:443');
    config.setNetworkExposure.mockRejectedValueOnce(new Error('The public address must be an https:// origin'));
    await mountIdentity();

    await act(async () => { fireEvent.click(screen.getByTestId('expose-proxy-save')); });
    expect(await screen.findByTestId('expose-refusal')).toHaveTextContent('must be an https:// origin');

    config.setNetworkExposure.mockResolvedValueOnce({ ...declared });
    await act(async () => { fireEvent.click(screen.getByTestId('expose-proxy-withdraw')); });
    // Withdrawing while exposed would leave plain HTTP: the exposure goes off with it.
    await waitFor(() => expect(config.setNetworkExposure).toHaveBeenLastCalledWith(false, { tls_terminated_by_proxy: false }));
  });

  it('leaves an environment declaration to the environment', async () => {
    config.getNetworkExposure.mockResolvedValue({ ...declared, tls_terminated_by_proxy: true, tls_declared_by_env: true, public_origin: 'https://kronn.example.com' });
    contacts.inviteCode.mockResolvedValue('kronns:me@kronn.example.com:443');
    await mountIdentity();
    expect(screen.getByTestId('expose-proxy-env')).toBeInTheDocument();
    expect(screen.queryByTestId('expose-proxy-save')).not.toBeInTheDocument();
  });
});

describe('IdentitySection — network exposure toggle', () => {
  it('reflects current exposure + toggling persists and surfaces the restart notice', async () => {
    config.getNetworkExposure.mockResolvedValue({ exposed: false, restart_required: false, port: 3140, reachable_ips: [], auth_token_set: false });
    config.setNetworkExposure.mockResolvedValue({ exposed: true, restart_required: true, port: 3140, reachable_ips: [], auth_token_set: true });
    const onExposureChange = vi.fn();
    await mountIdentity(vi.fn(), onExposureChange);

    const toggle = screen.getByTestId('expose-network-toggle');
    expect(toggle).toHaveAttribute('aria-checked', 'false');

    await act(async () => { fireEvent.click(toggle); });
    await waitFor(() => expect(config.setNetworkExposure).toHaveBeenCalledWith(true));

    // Exposing surfaces the security note + the restart-required notice.
    expect(screen.getByText('settings.exposeSecurityNote')).toBeInTheDocument();
    expect(screen.getByText('settings.exposeRestartRequired')).toBeInTheDocument();
    expect(toggle).toHaveAttribute('aria-checked', 'true');
    // The token card hears that exposing created the network access token.
    expect(onExposureChange).toHaveBeenCalledWith(expect.objectContaining({ exposed: true, auth_token_set: true }));
  });

  it('in Docker explains KRONN_BIND instead of offering an inert switch', async () => {
    config.getNetworkExposure.mockResolvedValue({ exposed: true, restart_required: false, in_docker: true, kronn_bind: '0.0.0.0', port: 3140, reachable_ips: [], auth_token_set: true });
    await mountIdentity();

    expect(screen.queryByTestId('expose-network-toggle')).not.toBeInTheDocument();
    expect(screen.getByTestId('expose-docker-note')).toBeInTheDocument();
    expect(screen.getByText('settings.networkReachable')).toBeInTheDocument();
    expect(screen.queryByText('settings.exposeRestartRequired')).not.toBeInTheDocument();
    // An exposed Docker instance keeps the security warning.
    expect(screen.getByText('settings.exposeSecurityNote')).toBeInTheDocument();
  });

  it('in Docker with KRONN_BIND on loopback shows no security note', async () => {
    config.getNetworkExposure.mockResolvedValue({ exposed: false, restart_required: false, in_docker: true, kronn_bind: '127.0.0.1', port: 3140, reachable_ips: [], auth_token_set: false });
    await mountIdentity();

    expect(screen.getByTestId('expose-docker-note')).toBeInTheDocument();
    expect(screen.getByText('settings.networkLocalOnly')).toBeInTheDocument();
    expect(screen.queryByText('settings.exposeSecurityNote')).not.toBeInTheDocument();
    expect(screen.queryByText('settings.exposeRestartRequired')).not.toBeInTheDocument();
  });

  it('saves the interface preferences server-side before the desktop restarts', async () => {
    const calls: string[] = [];
    flushUiPreferences.mockImplementation(async () => { calls.push('flush'); });
    const invoke = vi.fn(async (command: string) => { calls.push(command); });
    window.__TAURI__ = { core: { invoke: invoke as never } };
    try {
      config.getNetworkExposure.mockResolvedValue({ exposed: true, restart_required: true, port: 3140, reachable_ips: [], auth_token_set: false });
      await mountIdentity();
      await act(async () => { fireEvent.click(screen.getByText('settings.exposeRestartBtn')); });
      await waitFor(() => expect(calls).toEqual(['flush', 'restart_app']));
    } finally {
      delete window.__TAURI__;
    }
  });

  it('offers the desktop port only inside the desktop app', async () => {
    await mountIdentity();
    expect(screen.queryByTestId('desktop-port-field')).toBeNull();
    expect(config.getDesktopPort).not.toHaveBeenCalled();
    cleanup();
    window.__TAURI__ = { core: { invoke: vi.fn() as never } };
    try {
      await mountIdentity();
      expect(await screen.findByTestId('desktop-port-field')).toBeInTheDocument();
    } finally {
      delete window.__TAURI__;
    }
  });

  it('shows the CLI restart hint (not a Tauri button) in web mode', async () => {
    config.getNetworkExposure.mockResolvedValue({ exposed: true, restart_required: true, port: 3140, reachable_ips: [], auth_token_set: false });
    await mountIdentity();
    // happy-dom has no `__TAURI__` → web mode → CLI hint, no restart button.
    expect(screen.getByText('./kronn restart')).toBeInTheDocument();
    expect(screen.queryByText('settings.exposeRestartBtn')).toBeNull();
  });

  it('keeps P2P off until the operator turns it on, with the impersonation warning', async () => {
    await mountIdentity();
    const toggle = screen.getByTestId('p2p-toggle');
    await waitFor(() => expect(toggle).not.toBeDisabled());
    expect(toggle).toHaveAttribute('aria-checked', 'false');
    expect(screen.getByText('settings.p2pWarning')).toBeInTheDocument();
    await act(async () => { fireEvent.click(toggle); });
    expect(config.setServerConfig).toHaveBeenCalledWith({ p2p_enabled: true });
    await waitFor(() => expect(toggle).toHaveAttribute('aria-checked', 'true'));
  });

  it('saves the extra frontend origins on blur, one per line', async () => {
    await mountIdentity();
    const field = screen.getByTestId('frontend-origins');
    fireEvent.change(field, { target: { value: 'http://mac.tailnet.ts.net:3140\n\n http://192.168.1.5:3140 ' } });
    await act(async () => { fireEvent.blur(field); });
    expect(config.setServerConfig).toHaveBeenCalledWith({
      frontend_origins: ['http://mac.tailnet.ts.net:3140', 'http://192.168.1.5:3140'],
    });
  });
});

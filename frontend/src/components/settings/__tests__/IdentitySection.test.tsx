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

const { config, contacts } = vi.hoisted(() => ({
  config: {
    getServerConfig: vi.fn(),
    setServerConfig: vi.fn(),
    getGlobalContext: vi.fn(),
    saveGlobalContext: vi.fn(),
    getGlobalContextMode: vi.fn(),
    saveGlobalContextMode: vi.fn(),
  },
  contacts: {
    networkInfo: vi.fn(),
  },
}));

vi.mock('../../../lib/api', () => ({ config, contacts }));

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
  contacts.networkInfo.mockResolvedValue({
    tailscale_ip: null, advertised_host: null, detected_ips: [],
  });
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

async function mountIdentity(toast = vi.fn()) {
  let result: ReturnType<typeof render>;
  await act(async () => {
    result = render(<IdentitySection toast={toast as never} t={t} />);
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
    expect((screen.getByPlaceholderText('Ex: JohnDoe42') as HTMLInputElement).value).toBe('JohnDoe42');
    expect((screen.getByPlaceholderText('email@example.com') as HTMLInputElement).value).toBe('john@example.com');
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
    expect((screen.getByPlaceholderText('Ex: JohnDoe42') as HTMLInputElement).value).toBe('');
  });
});

describe('IdentitySection — save-on-change', () => {
  it('typing in pseudo fires setServerConfig with a partial payload', async () => {
    await mountIdentity();
    const input = screen.getByPlaceholderText('Ex: JohnDoe42') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'Romuald' } });
    expect(config.setServerConfig).toHaveBeenCalledWith({ pseudo: 'Romuald' });
    expect(input.value).toBe('Romuald');
  });

  it('typing in avatar email fires setServerConfig with avatar_email only', async () => {
    await mountIdentity();
    const input = screen.getByPlaceholderText('email@example.com') as HTMLInputElement;
    fireEvent.change(input, { target: { value: 'me@example.com' } });
    expect(config.setServerConfig).toHaveBeenCalledWith({ avatar_email: 'me@example.com' });
  });

  it('typing in bio fires setServerConfig with bio only', async () => {
    await mountIdentity();
    const ta = screen.getByPlaceholderText('settings.bioPlaceholder') as HTMLTextAreaElement;
    fireEvent.change(ta, { target: { value: 'Eng @ Euronews' } });
    expect(config.setServerConfig).toHaveBeenCalledWith({ bio: 'Eng @ Euronews' });
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

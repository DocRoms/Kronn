// Configuration's anchors behind the real router and the real pages, with only
// the API boundary simulated: the run-retention banner of Automation and of
// Configuration itself, an arrival on `/config#<anchor>` while the page is
// still loading, and the section links of the page's side menu.
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { withDashboardRoutes } from '../../test/routerWrapper';
import { tourAlreadyTaken } from '../../test/tour';

import type * as RealApi from '../../lib/api';
import type { ServerConfigPublic } from '../../types/generated';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));
/** API methods the pages called that this suite does not declare. */
const unexpectedCalls = vi.hoisted((): string[] => []);
/** The server's configuration, retention off: the database keeps every run
 *  output, so the banners show. */
const serverConfig = vi.hoisted((): ServerConfigPublic => ({
  host: 'localhost', port: 3140, domain: null, max_concurrent_agents: 5, agent_stall_timeout_min: 10,
  agent_global_timeout_min: 60, local_agent_global_timeout_min: 120, auth_enabled: false, pseudo: null,
  avatar_email: null, bio: null, debug_mode: false, discussion_notes_enabled: false, default_model_tier: 'default',
  default_summary_strategy: 'Off', agent_handoffs_enabled: false, agent_handoff_paid_limit: 1,
  agent_handoff_paid_unlimited: false, agent_handoff_blocked_agents: [],
  discussion_weight: { enabled: true, amber_bytes: 512 * 1024, red_bytes: 2 * 1024 * 1024 },
  execution_variable_retention_days: 30, run_payload_retention_days: 0, p2p_enabled: false, frontend_origins: [],
}));

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  const api = buildApiMock();
  api.config.getServerConfig = vi.fn<typeof RealApi.config.getServerConfig>().mockResolvedValue(serverConfig);
  api.config.dbUsage = vi.fn<typeof RealApi.config.dbUsage>().mockResolvedValue({
    file_bytes: 534 * 1024 * 1024, wal_bytes: 0, free_bytes: 0, tables: [],
  });
  api.config.dbInfo = vi.fn<typeof RealApi.config.dbInfo>().mockResolvedValue({
    size_bytes: 534 * 1024 * 1024, project_count: 0, discussion_count: 0, message_count: 0, mcp_count: 0,
    workflow_count: 0, workflow_run_count: 0, custom_skill_count: 0, custom_profile_count: 0, custom_directive_count: 0,
  });
  // What the real pages read beyond the shared mock, declared with the shape
  // of the real API, so a branch runs on the answer it would get.
  const declared: { [N in keyof typeof RealApi]?: { [M in keyof (typeof RealApi)[N]]?: unknown } } = {
    agents: {
      detect: vi.fn<typeof RealApi.agents.detect>().mockResolvedValue([]),
      quotaStates: vi.fn<typeof RealApi.agents.quotaStates>().mockResolvedValue([]),
    },
    config: {
      getAntiHallucinationMode: vi.fn<typeof RealApi.config.getAntiHallucinationMode>().mockResolvedValue('warn'),
      getEmbedOrigins: vi.fn<typeof RealApi.config.getEmbedOrigins>().mockResolvedValue([]),
    },
    contacts: {
      networkInfo: vi.fn<typeof RealApi.contacts.networkInfo>().mockResolvedValue({
        tailscale_ip: null, advertised_host: 'localhost', port: 3140, domain: null, detected_ips: [],
      }),
    },
    discussions: {
      getRunning: vi.fn<typeof RealApi.discussions.getRunning>().mockResolvedValue([]),
    },
    workflows: {
      autoDisabled: vi.fn<typeof RealApi.workflows.autoDisabled>().mockResolvedValue([]),
    },
  };
  for (const [name, methods] of Object.entries(declared)) {
    Object.assign((api as unknown as Record<string, Record<string, unknown>>)[name], methods);
  }
  // Anything else the pages call is a gap in this simulation: it is recorded,
  // rejected, and fails the test (see `afterEach`) instead of being answered.
  const guarded = (namespace: Record<string, unknown>, name: string) => new Proxy(namespace, {
    get: (target, key) => {
      if (typeof key !== 'string' || key in target || key === 'then') return Reflect.get(target, key);
      return (..._args: unknown[]) => {
        unexpectedCalls.push(`${name}.${key}`);
        return Promise.reject(new Error(`unexpected API call: ${name}.${key}`));
      };
    },
  });
  return Object.fromEntries(Object.entries(api).map(([name, value]) => [
    name, value && typeof value === 'object' && !Array.isArray(value) ? guarded(value as Record<string, unknown>, name) : value,
  ]));
});

import { config as configApi, pages as pagesApi } from '../../lib/api';
import { Dashboard } from '../../pages/Dashboard';

const scrolled = vi.fn();

async function renderDashboard(initialPath: string, historyState: unknown = null) {
  await act(async () => {
    render(<I18nProvider>{withDashboardRoutes(<Dashboard onReset={vi.fn()} />, initialPath, historyState)}</I18nProvider>);
  });
}

/** A reload: the same address and the same history entry, a fresh page. */
async function reload() {
  const address = `${window.location.pathname}${window.location.search}${window.location.hash}`;
  const entry: unknown = window.history.state;
  cleanup();
  scrolled.mockReset();
  await renderDashboard(address, entry);
}

const sectionLink = (id: string) => waitFor(() => {
  const found = document.querySelector<HTMLAnchorElement>(`nav.set-nav a[href="/config#${id}"]`);
  expect(found).not.toBeNull();
  return found!;
});
const highlighted = () => document.querySelector('nav.set-nav a[aria-current="location"]')?.getAttribute('href');

const retentionSetting = () => document.getElementById('run-payload-retention') as HTMLSelectElement | null;
const retentionLink = () => screen.getAllByTestId('run-retention-banner')[0].querySelector<HTMLAnchorElement>('a.rr-btn')!;

beforeEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  tourAlreadyTaken();
  vi.mocked(pagesApi.capability).mockResolvedValue({ activated: false, activated_at: null });
  scrolled.mockReset();
  Element.prototype.scrollIntoView = function (this: Element, options?: ScrollIntoViewOptions | boolean) {
    scrolled(this.id, options);
  };
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
  const unexpected = unexpectedCalls.splice(0);
  expect(unexpected, 'API calls this suite does not declare').toEqual([]);
});

describe('Configuration anchors — the run-retention setting', () => {
  it('Automation\'s banner links to the setting\'s address, which centres and focuses it once loaded', async () => {
    await renderDashboard('/workflows');
    const link = await waitFor(() => retentionLink());
    expect(link).toHaveAttribute('href', '/config#run-payload-retention');

    await act(async () => { fireEvent.click(link); });

    await waitFor(() => expect(window.location.pathname).toBe('/config'));
    expect(window.location.hash).toBe('#run-payload-retention');
    // No hand-off through storage: the address carries the target.
    expect(sessionStorage.length).toBe(0);
    await waitFor(() => expect(document.activeElement).toBe(retentionSetting()), { timeout: 3000 });
    expect(retentionSetting()).not.toBeDisabled();
    expect(scrolled).toHaveBeenCalledWith('run-payload-retention', { behavior: 'smooth', block: 'center' });
  });

  it('waits for the setting to be enabled before pointing at it, on a direct arrival', async () => {
    let deliver!: (value: ServerConfigPublic) => void;
    vi.mocked(configApi.getServerConfig).mockReturnValue(new Promise<ServerConfigPublic>(resolve => { deliver = resolve; }));
    await renderDashboard('/config#run-payload-retention');

    await waitFor(() => expect(retentionSetting()).toBeInTheDocument());
    expect(retentionSetting()).toBeDisabled();
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 400)); });
    expect(scrolled).not.toHaveBeenCalledWith('run-payload-retention', expect.anything());

    await act(async () => {
      deliver(serverConfig);
    });
    await waitFor(() => expect(document.activeElement).toBe(retentionSetting()), { timeout: 3000 });
  });

  it('highlights the setting\'s own section in the side menu, until the reader scrolls', async () => {
    await renderDashboard('/config#run-payload-retention');
    await waitFor(() => expect(document.activeElement).toBe(retentionSetting()), { timeout: 3000 });
    const current = () => document.querySelector('nav.set-nav a[aria-current="location"]')?.getAttribute('href');

    // The scroll that brought the setting in ends at the bottom of the page,
    // where the spy alone would name the last section.
    await act(async () => { window.dispatchEvent(new Event('scroll')); });
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 50)); });
    expect(current()).toBe('/config#settings-database');

    // The reader scrolls on their own: the spy follows the page again.
    await act(async () => {
      window.dispatchEvent(new Event('wheel'));
      window.dispatchEvent(new Event('scroll'));
    });
    await waitFor(() => expect(current()).toBe('/config#settings-general'));
  });

  it('Configuration\'s own banner moves the address in place and points at the setting', async () => {
    await renderDashboard('/config');
    const depth = window.history.length;
    const link = await waitFor(() => retentionLink());

    await act(async () => { fireEvent.click(link); });

    expect(window.location.hash).toBe('#run-payload-retention');
    expect(window.history.length).toBe(depth);
    await waitFor(() => expect(document.activeElement).toBe(retentionSetting()), { timeout: 3000 });
  });
});

describe('Configuration anchors — the side menu', () => {
  it('makes every section a link to its address; a plain click scrolls here and replaces the address', async () => {
    await renderDashboard('/config');
    const depth = window.history.length;
    const server = await waitFor(() => {
      const found = document.querySelector<HTMLAnchorElement>('nav.set-nav a[href="/config#settings-server"]');
      expect(found).not.toBeNull();
      return found;
    });
    expect(server).not.toBeNull();

    await act(async () => { fireEvent.click(server!); });
    expect(window.location.hash).toBe('#settings-server');
    expect(window.history.length).toBe(depth);
    // The page scrolled itself once; the route did not scroll again on top of it.
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 500)); });
    expect(scrolled.mock.calls.filter(([id]) => id === 'settings-server')).toHaveLength(1);

    // A modified click is left to the browser (its own new tab). jsdom would
    // follow the fragment in this tab instead, so nothing more is asserted.
    const modified = new MouseEvent('click', { bubbles: true, cancelable: true, ctrlKey: true });
    server!.dispatchEvent(modified);
    expect(modified.defaultPrevented).toBe(false);
  });

  // Review #227 — the page's own scroll is said for that one move only: the
  // entry it leaves behind is an address like any other.
  it('brings the section back into view on a reload after a click, highlighted', async () => {
    await renderDashboard('/config');
    await act(async () => { fireEvent.click(await sectionLink('settings-server')); });
    expect(window.location.hash).toBe('#settings-server');

    await reload();

    await waitFor(() => expect(scrolled).toHaveBeenCalledWith('settings-server', { behavior: 'smooth', block: 'start' }), { timeout: 3000 });
    await waitFor(() => expect(highlighted()).toBe('/config#settings-server'));
  });

  it('brings the section back into view on a reload while the page is still loading', async () => {
    await renderDashboard('/config');
    await act(async () => { fireEvent.click(await sectionLink('settings-server')); });

    let deliver!: (value: ServerConfigPublic) => void;
    vi.mocked(configApi.getServerConfig).mockReturnValue(new Promise<ServerConfigPublic>(resolve => { deliver = resolve; }));
    await reload();
    await act(async () => {
      deliver(serverConfig);
    });

    await waitFor(() => expect(scrolled).toHaveBeenCalledWith('settings-server', { behavior: 'smooth', block: 'start' }), { timeout: 3000 });
    await waitFor(() => expect(highlighted()).toBe('/config#settings-server'));
  });

  it('brings the section into view again when Back then Forward return to it', async () => {
    await renderDashboard('/planning');
    await act(async () => { fireEvent.click(document.querySelector<HTMLElement>('[data-tour-id="nav-settings"]')!); });
    await waitFor(() => expect(window.location.pathname).toBe('/config'));
    await act(async () => { fireEvent.click(await sectionLink('settings-server')); });
    expect(window.location.hash).toBe('#settings-server');

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(window.location.pathname).toBe('/planning'));
    scrolled.mockReset();
    await act(async () => { window.history.forward(); });
    await waitFor(() => expect(window.location.hash).toBe('#settings-server'));

    await waitFor(() => expect(scrolled).toHaveBeenCalledWith('settings-server', { behavior: 'smooth', block: 'start' }), { timeout: 3000 });
  });
});

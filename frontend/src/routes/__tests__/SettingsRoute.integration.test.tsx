// Configuration's anchors behind the real router and the real pages, with only
// the API boundary simulated: the run-retention banner of Automation and of
// Configuration itself, an arrival on `/config#<anchor>` while the page is
// still loading, and the section links of the page's side menu.
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { withDashboardRoutes } from '../../test/routerWrapper';
import { tourAlreadyTaken } from '../../test/tour';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));
vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  const api = buildApiMock();
  // Retention off: the database keeps every run output, the banners show.
  api.config.getServerConfig = vi.fn().mockResolvedValue({
    pseudo: null, avatar_email: null, host: 'localhost', port: 3140, default_model_tier: 'default',
    default_summary_strategy: 'Off', agent_handoffs_enabled: false, agent_handoff_paid_limit: 1,
    agent_handoff_paid_unlimited: false, agent_handoff_blocked_agents: [], execution_variable_retention_days: 30,
    run_payload_retention_days: 0,
  });
  api.config.dbUsage = vi.fn().mockResolvedValue({ file_bytes: 534 * 1024 * 1024 });
  api.config.dbInfo = vi.fn().mockResolvedValue(null);
  // The real Configuration page reads many endpoints the shared mock does not
  // list; one it does not know resolves to an empty list rather than throwing.
  const lenient = (namespace: Record<string, unknown>) => new Proxy(namespace, {
    get: (target, key: string) => {
      if (!(key in target)) target[key] = vi.fn().mockResolvedValue([]);
      return target[key];
    },
  });
  return Object.fromEntries(Object.entries(api).map(([name, value]) => [
    name, value && typeof value === 'object' && !Array.isArray(value) ? lenient(value as Record<string, unknown>) : value,
  ]));
});

import { config as configApi, pages as pagesApi } from '../../lib/api';
import { Dashboard } from '../../pages/Dashboard';

const scrolled = vi.fn();

async function renderDashboard(initialPath: string) {
  await act(async () => {
    render(<I18nProvider>{withDashboardRoutes(<Dashboard onReset={vi.fn()} />, initialPath)}</I18nProvider>);
  });
}

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
    let deliver!: (value: unknown) => void;
    vi.mocked(configApi.getServerConfig).mockReturnValue(new Promise(resolve => { deliver = resolve; }) as never);
    await renderDashboard('/config#run-payload-retention');

    await waitFor(() => expect(retentionSetting()).toBeInTheDocument());
    expect(retentionSetting()).toBeDisabled();
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 400)); });
    expect(scrolled).not.toHaveBeenCalledWith('run-payload-retention', expect.anything());

    await act(async () => {
      deliver({ host: 'localhost', port: 3140, run_payload_retention_days: 0, execution_variable_retention_days: 30, agent_handoff_blocked_agents: [] });
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
});

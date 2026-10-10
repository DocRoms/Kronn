// The Automation page behind the real router and the real dashboard shell,
// with only the API boundary simulated: leaving Automation with a resource
// open and coming back through the nav, an explicit address, Back/Forward,
// and a remembered resource that no longer exists.
import { act, cleanup, fireEvent, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../lib/I18nContext';
import { withDashboardRoutes } from '../../test/routerWrapper';
import { tourAlreadyTaken } from '../../test/tour';
import type { QuickPrompt } from '../../types/generated';

vi.mock('../../hooks/useWebSocket', () => ({
  useWebSocket: vi.fn(() => ({ connected: false, connectionState: 'connecting' })),
}));
vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { pages as pagesApi, quickPrompts as quickPromptsApi } from '../../lib/api';
import { Dashboard } from '../../pages/Dashboard';

const quickPrompt = (id: string, name: string): QuickPrompt => ({
  id, name, icon: '✨', description: '', prompt_template: name, variables: [], agent: 'ClaudeCode', project_id: null,
  skill_ids: [], profile_ids: [], directive_ids: [], tier: 'default', pinned: false,
  created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z',
});

async function renderDashboard(initialPath: string) {
  await act(async () => {
    render(<I18nProvider>{withDashboardRoutes(<Dashboard onReset={vi.fn()} />, initialPath)}</I18nProvider>);
  });
}

async function clickNav(page: string) {
  await act(async () => { fireEvent.click(document.querySelector<HTMLElement>(`[data-tour-id="nav-${page}"]`)!); });
}

const openQuickPrompt = () => document.querySelector<HTMLElement>('.automation-viewer .qp-card[data-detail="true"]');

beforeEach(() => {
  localStorage.clear();
  tourAlreadyTaken();
  vi.mocked(pagesApi.capability).mockResolvedValue({ activated: false, activated_at: null });
  vi.mocked(quickPromptsApi.list).mockResolvedValue([quickPrompt('qp-1', 'Release notes'), quickPrompt('qp-2', 'Changelog')]);
});

afterEach(() => {
  cleanup();
  localStorage.clear();
  vi.clearAllMocks();
});

describe('Automation route — the last visit', () => {
  it('reopens the Quick Prompt left open when the nav brings the reader back', async () => {
    await renderDashboard('/workflows/qp/qp-1');
    await waitFor(() => expect(openQuickPrompt()).toHaveTextContent('Release notes'));
    const depth = window.history.length;

    await clickNav('planning');
    expect(window.location.pathname).toBe('/planning');
    expect(openQuickPrompt()).toBeNull();

    await clickNav('workflows');
    await waitFor(() => expect(window.location.pathname).toBe('/workflows/qp/qp-1'));
    await waitFor(() => expect(openQuickPrompt()).toHaveTextContent('Release notes'));
    // Planning, then Automation: the bare address was replaced, not kept.
    expect(window.history.length).toBe(depth + 2);

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(window.location.pathname).toBe('/planning'));
    await act(async () => { window.history.forward(); });
    await waitFor(() => expect(window.location.pathname).toBe('/workflows/qp/qp-1'));
    await waitFor(() => expect(openQuickPrompt()).toHaveTextContent('Release notes'));
  });

  it('lets an explicit address win over the last visit', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'quickPrompts', resourceId: 'qp-1' }));
    await renderDashboard('/workflows/qp/qp-2');

    await waitFor(() => expect(openQuickPrompt()).toHaveTextContent('Changelog'));
    expect(window.location.pathname).toBe('/workflows/qp/qp-2');
  });

  it('lets go of a remembered Quick Prompt that no longer exists, in place of the address', async () => {
    localStorage.setItem('kronn:automationNavigation', JSON.stringify({ tab: 'quickPrompts', resourceId: 'qp-gone' }));
    const depth = window.history.length;
    await renderDashboard('/workflows');

    await waitFor(() => expect(window.location.pathname).toBe('/workflows/qp'));
    expect(window.history.length).toBe(depth);
    expect(openQuickPrompt()).toBeNull();
  });

  it('makes choosing the type list from a Quick Prompt a step Back undoes', async () => {
    await renderDashboard('/workflows/qp/qp-1');
    await waitFor(() => expect(openQuickPrompt()).toHaveTextContent('Release notes'));
    const depth = window.history.length;

    fireEvent.click(document.querySelector<HTMLElement>('[data-tour-id="automation-filter-type"]')!);
    await act(async () => { fireEvent.click(document.querySelector<HTMLElement>('[data-kind-option="quickPrompts"]')!); });
    await waitFor(() => expect(window.location.pathname).toBe('/workflows/qp'));
    expect(window.history.length).toBe(depth + 1);
    expect(openQuickPrompt()).toBeNull();

    await act(async () => { window.history.back(); });
    await waitFor(() => expect(window.location.pathname).toBe('/workflows/qp/qp-1'));
    await waitFor(() => expect(openQuickPrompt()).toHaveTextContent('Release notes'));

    await act(async () => { window.history.forward(); });
    await waitFor(() => expect(window.location.pathname).toBe('/workflows/qp'));
    await waitFor(() => expect(openQuickPrompt()).toBeNull());
  });
});

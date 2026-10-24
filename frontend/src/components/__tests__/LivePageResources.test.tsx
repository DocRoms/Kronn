// KT-1450 — the resources behind a Page: per workflow, what each lane loads,
// a promotion that names its scope and confirms what changed, and a failed
// read that says so instead of hiding the panel.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, render, screen, fireEvent, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { I18nProvider } from '../../lib/I18nContext';
import type { LivePageResource } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { LivePageResources } from '../LivePageResources';
import { pages as pagesApi, workflows as workflowsApi } from '../../lib/api';

const stepUse = (step: string, normal: string | null, shadow: string | null) => ({ workflow_id: 'wf-1', workflow_name: 'PR review', step_name: step, normal, shadow });
const resources: LivePageResource[] = [
  { kind: 'skill', id: 'custom-review', name: 'Review', latest_version: 5, uses: [stepUse('reason', 'custom-review@4', 'custom-review@5'), stepUse('check', 'custom-review', 'custom-review@5')] },
  { kind: 'quick_prompt', id: 'qp-1', name: 'Summary', latest_version: 3, uses: [stepUse('sum', null, 'v2')] },
  { kind: 'skill', id: 'rust', name: 'Rust', latest_version: null, uses: [stepUse('reason', 'rust', null)] },
];
const renderPanel = (pageId = 'page-1') => render(<MemoryRouter><I18nProvider><LivePageResources pageId={pageId} /></I18nProvider></MemoryRouter>);

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(pagesApi.resources).mockResolvedValue(resources);
});
afterEach(() => { vi.unstubAllGlobals(); });

describe('LivePageResources (KT-1450)', () => {
  it('groups the steps of a workflow and names versions, not raw ids', async () => {
    renderPanel();
    const skill = await screen.findByTestId('live-page-resource-use-custom-review-wf-1');
    expect(skill.textContent).toContain('reason, check');
    expect(skill.textContent).toContain('reason : v4 épinglée · check : dernière (v5)');
    expect(skill.textContent).not.toContain('custom-review@');
    expect(screen.getAllByTestId('live-page-resource-promote-custom-review')).toHaveLength(1);
    expect(screen.getByTestId('live-page-resource-use-rust-wf-1').textContent).toContain('sans versions');
  });

  it('promotes once, after a confirmation naming the steps, then says what changed', async () => {
    const confirm = vi.fn();
    vi.stubGlobal('confirm', confirm);
    let release: () => void = () => {};
    vi.mocked(workflowsApi.promoteShadow).mockImplementation(() => new Promise(resolve => { release = () => resolve(null as never); }));
    renderPanel();
    const promote = await screen.findByTestId('live-page-resource-promote-custom-review');

    confirm.mockReturnValueOnce(false);
    fireEvent.click(promote);
    expect(workflowsApi.promoteShadow).not.toHaveBeenCalled();
    await act(async () => {});

    confirm.mockReturnValue(true);
    fireEvent.click(promote);
    fireEvent.click(promote);
    expect(confirm.mock.calls[1][0]).toContain('reason, check');
    await waitFor(() => expect(workflowsApi.promoteShadow).toHaveBeenCalledTimes(1));
    expect(workflowsApi.promoteShadow).toHaveBeenCalledWith('wf-1', { kind: 'skill', id: 'custom-review' });
    await act(async () => { release(); });
    expect(await screen.findByTestId('live-page-resources-done')).toHaveTextContent('les runs normaux de « PR review » chargent maintenant v5');
    await waitFor(() => expect(pagesApi.resources).toHaveBeenCalledTimes(2));
  });

  it('warns that a Quick Prompt promotion changes it everywhere', async () => {
    const confirm = vi.fn().mockReturnValue(false);
    vi.stubGlobal('confirm', confirm);
    renderPanel();
    fireEvent.click(await screen.findByTestId('live-page-resource-promote-qp-1'));
    expect(confirm.mock.calls[0][0]).toContain('pas seulement « PR review »');
  });

  it('offers no promotion when the shadow tests the version the normal run already loads', async () => {
    vi.mocked(pagesApi.resources).mockResolvedValue([
      { kind: 'skill', id: 'custom-review', name: 'Review', latest_version: 5, uses: [stepUse('reason', 'custom-review', 'custom-review@5')] },
    ]);
    renderPanel();
    const row = await screen.findByTestId('live-page-resource-use-custom-review-wf-1');
    expect(row.textContent).toContain('le shadow teste la même version');
    expect(screen.queryByTestId('live-page-resource-promote-custom-review')).toBeNull();
  });

  it('points to shadow runs when nothing is tested yet', async () => {
    vi.mocked(pagesApi.resources).mockResolvedValue([resources[2]]);
    renderPanel();
    await screen.findByTestId('live-page-resources');
    expect(screen.getByText('configurer « PR review »')).toHaveAttribute('href', '/workflows/wf-1/edit');
  });

  it('shows nothing for a page without resources', async () => {
    vi.mocked(pagesApi.resources).mockResolvedValue([]);
    const { container } = renderPanel('page-2');
    await waitFor(() => expect(pagesApi.resources).toHaveBeenCalled());
    expect(container.querySelector('[data-testid="live-page-resources"]')).toBeNull();
  });

  it('says when the resources cannot be read, and retries', async () => {
    vi.mocked(pagesApi.resources).mockRejectedValueOnce(new Error('down'));
    renderPanel();
    const error = await screen.findByTestId('live-page-resources-error');
    expect(error).toHaveAttribute('role', 'alert');
    fireEvent.click(screen.getByRole('button', { name: /Réessayer/ }));
    expect(await screen.findByTestId('live-page-resources')).toBeInTheDocument();
  });
});

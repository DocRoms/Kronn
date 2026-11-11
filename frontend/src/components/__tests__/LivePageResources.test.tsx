// KT-1450, KT-1459 — the resources behind a Page: per step, what each shadow loads,
// a promotion that names its step and confirms what changed, and a failed
// read that says so instead of hiding the panel.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { act, render, screen, fireEvent, waitFor, within } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { I18nProvider } from '../../lib/I18nContext';
import type { LivePageResource, WorkflowStep } from '../../types/generated';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { LivePageResources } from '../LivePageResources';
import { pages as pagesApi, workflows as workflowsApi, skills as skillsApi } from '../../lib/api';

const stepUse = (step: string, normal: string | null, shadow: string | null, shadowId = 'candidate') => ({
  workflow_id: 'wf-1', workflow_name: 'PR review', step_name: step, normal,
  shadows: shadow == null ? [] : [{ shadow_id: shadowId, label: shadowId === 'candidate' ? 'Candidate' : shadowId, version: shadow }],
});
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
  it('lists each step on its own row and names versions, not raw ids', async () => {
    renderPanel();
    const rows = await screen.findAllByTestId('live-page-resource-use-custom-review-wf-1');
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('étape : reason');
    expect(rows[0].textContent).toContain('v4 épinglée');
    expect(rows[1].textContent).toContain('étape : check');
    expect(rows[1].textContent).toContain('dernière (v5)');
    expect(rows[0].textContent).not.toContain('custom-review@');
    // `check` loads the latest version, which the shadow tests too: nothing to promote there.
    expect(screen.getAllByTestId('live-page-resource-promote-custom-review-candidate')).toHaveLength(1);
    expect(screen.getByTestId('live-page-resource-use-rust-wf-1').textContent).toContain('sans versions');
  });

  it('promotes once, after a confirmation naming the step, then says what changed', async () => {
    const confirm = vi.fn();
    vi.stubGlobal('confirm', confirm);
    let release: () => void = () => {};
    vi.mocked(workflowsApi.promoteShadow).mockImplementation(() => new Promise(resolve => { release = () => resolve(null as never); }));
    renderPanel();
    const promote = await screen.findByTestId('live-page-resource-promote-custom-review-candidate');

    confirm.mockReturnValueOnce(false);
    fireEvent.click(promote);
    expect(workflowsApi.promoteShadow).not.toHaveBeenCalled();
    await act(async () => {});

    confirm.mockReturnValue(true);
    fireEvent.click(promote);
    fireEvent.click(promote);
    expect(confirm.mock.calls[1][0]).toContain('L’étape « reason » de « PR review »');
    await waitFor(() => expect(workflowsApi.promoteShadow).toHaveBeenCalledTimes(1));
    expect(workflowsApi.promoteShadow).toHaveBeenCalledWith('wf-1', { kind: 'skill', id: 'custom-review', step: 'reason', shadow: 'candidate' });
    await act(async () => { release(); });
    expect(await screen.findByTestId('live-page-resources-done')).toHaveTextContent('l’étape de « PR review » charge désormais v5');
    await waitFor(() => expect(pagesApi.resources).toHaveBeenCalledTimes(2));
  });

  it('says a Quick Prompt promotion pins this step and leaves the Quick Prompt as it is', async () => {
    const confirm = vi.fn().mockReturnValue(false);
    vi.stubGlobal('confirm', confirm);
    renderPanel();
    fireEvent.click(await screen.findByTestId('live-page-resource-promote-qp-1-candidate'));
    expect(confirm.mock.calls[0][0]).toContain('L’étape « sum » de « PR review » épinglera');
    expect(confirm.mock.calls[0][0]).toContain('Le Quick Prompt lui-même ne change pas');
  });

  it('offers no promotion when the shadow tests the version the step already loads', async () => {
    vi.mocked(pagesApi.resources).mockResolvedValue([
      { kind: 'skill', id: 'custom-review', name: 'Review', latest_version: 5, uses: [stepUse('reason', 'custom-review', 'custom-review@5')] },
    ]);
    renderPanel();
    const row = await screen.findByTestId('live-page-resource-use-custom-review-wf-1');
    expect(row.textContent).toContain('le shadow teste la même version');
    expect(screen.queryByTestId('live-page-resource-promote-custom-review-candidate')).toBeNull();
  });

  it('points to the step shadows when nothing is tested yet', async () => {
    vi.mocked(pagesApi.resources).mockResolvedValue([resources[2]]);
    renderPanel();
    await screen.findByTestId('live-page-resources');
    expect(screen.getByText('configurer « PR review »')).toHaveAttribute('href', '/workflows/wf-1/edit#shadow');
  });

  it('promotes the version of the shadow it names when several shadows test one skill', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    vi.mocked(workflowsApi.promoteShadow).mockResolvedValue(null as never);
    vi.mocked(pagesApi.resources).mockResolvedValue([
      { kind: 'skill', id: 'custom-review', name: 'Review', latest_version: 13, uses: [
        { ...stepUse('reason', 'custom-review@11', null), shadows: [
          { shadow_id: 'pr-review-13', label: 'pr-review@13', version: 'custom-review@13' },
          { shadow_id: 'no-skill', label: 'No skill', version: '' },
        ] },
      ] },
    ]);
    renderPanel();
    const row = await screen.findByTestId('live-page-resource-use-custom-review-wf-1');
    expect(row.textContent).toContain('pr-review@13');
    expect(row.textContent).toContain('No skill');
    fireEvent.click(screen.getByTestId('live-page-resource-promote-custom-review-pr-review-13'));
    await waitFor(() => expect(workflowsApi.promoteShadow).toHaveBeenCalledWith('wf-1', { kind: 'skill', id: 'custom-review', step: 'reason', shadow: 'pr-review-13' }));
    // A shadow that drops the skill offers its promotion too.
    expect(screen.getByTestId('live-page-resource-promote-custom-review-no-skill')).toBeInTheDocument();
  });

  it('creates, edits and removes a shadow from the panel, saving the steps of the workflow', async () => {
    vi.stubGlobal('confirm', vi.fn().mockReturnValue(true));
    const history = [5, 4, 3].map(version_index => ({ skill_id: 'custom-review', version_index, name: 'Review', description: '', icon: '', category: 'Domain', content: '', created_at: '', current: version_index === 5 }));
    vi.mocked(skillsApi.history).mockResolvedValue(history as never);
    vi.mocked(pagesApi.resources).mockResolvedValue([{ ...resources[0], uses: [stepUse('reason', 'custom-review@4', 'custom-review@5')] }]);
    const candidate = { id: 'candidate', label: 'Candidate', enabled: true, skills: { 'custom-review': 'custom-review@5' } };
    const steps = [
      { name: 'reason', shadows: [candidate] },
      { name: 'other' },
    ] as unknown as WorkflowStep[];
    vi.mocked(workflowsApi.get).mockResolvedValue({ id: 'wf-1', steps } as never);
    vi.mocked(workflowsApi.update).mockResolvedValue(null as never);
    renderPanel();
    const panel = await screen.findByTestId('live-page-resource-shadows-custom-review-wf-1-reason');
    fireEvent.click(panel.querySelector('summary')!);

    // Test another version in a new shadow of that step; the other step is untouched.
    const adder = within(panel).getByLabelText('Version à tester');
    await waitFor(() => expect(within(adder).getAllByRole('option').length).toBeGreaterThan(2));
    fireEvent.change(adder, { target: { value: '3' } });
    fireEvent.click(screen.getByTestId('live-page-resource-shadow-add-custom-review-wf-1-reason'));
    await waitFor(() => expect(workflowsApi.update).toHaveBeenCalledTimes(1));
    expect(workflowsApi.update).toHaveBeenLastCalledWith('wf-1', { steps: [
      { name: 'reason', shadows: [candidate, { id: 'shadow-2', label: 'Review v3', enabled: true, skills: { 'custom-review': 'custom-review@3' } }] },
      { name: 'other' },
    ] });

    // Change what the existing shadow tests.
    const row = within(screen.getByTestId('live-page-resource-shadow-custom-review-candidate'));
    fireEvent.change(row.getByRole('combobox'), { target: { value: '4' } });
    await waitFor(() => expect(workflowsApi.update).toHaveBeenCalledTimes(2));
    expect(workflowsApi.update).toHaveBeenLastCalledWith('wf-1', { steps: [
      { name: 'reason', shadows: [{ ...candidate, skills: { 'custom-review': 'custom-review@4' } }] },
      { name: 'other' },
    ] });

    // Remove it, once confirmed.
    fireEvent.click(screen.getByTestId('live-page-resource-shadow-remove-custom-review-candidate'));
    await waitFor(() => expect(workflowsApi.update).toHaveBeenCalledTimes(3));
    expect(workflowsApi.update).toHaveBeenLastCalledWith('wf-1', { steps: [{ name: 'reason', shadows: [] }, { name: 'other' }] });
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

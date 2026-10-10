import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageDatasetUsage, LivePageDatasetView } from '../../types/generated';

vi.mock('../../lib/api', () => ({
  pages: { publish: vi.fn(), deleteDataset: vi.fn(), updateDataset: vi.fn() },
}));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ locale: 'en', t: (key: string, ...args: unknown[]) => [key, ...args].join('|') }),
}));

import { pages as pagesApi } from '../../lib/api';
import { ApiRequestError } from '../../lib/apiRequestError';
import { LivePageDatasetControls, LivePageDatasetUsageLine } from '../LivePageDatasetControls';

function dataset(overrides: Partial<LivePageDatasetView> = {}): LivePageDatasetView {
  return {
    id: 'ds-1', page_id: 'page-1', name: 'auto_reviews', kind: 'time_series', current: null, schema: null,
    max_points: 50000, max_age_days: null, updated_at: '2026-09-14T10:00:00Z', points: [], data_size_bytes: 71000,
    ...overrides,
  };
}

function setup(overrides: Partial<LivePageDatasetView> = {}) {
  const onChanged = vi.fn(() => Promise.resolve());
  const onDeleted = vi.fn();
  render(<LivePageDatasetControls pageId="page-1" dataset={dataset(overrides)} onChanged={onChanged} onDeleted={onDeleted} />);
  return { onChanged, onDeleted };
}

const confirm = vi.fn<(message?: string) => boolean>();

beforeEach(() => {
  vi.mocked(pagesApi.publish).mockReset().mockResolvedValue({} as never);
  vi.mocked(pagesApi.deleteDataset).mockReset().mockResolvedValue({} as never);
  vi.mocked(pagesApi.updateDataset).mockReset().mockResolvedValue({} as never);
  confirm.mockReset().mockReturnValue(true);
  vi.stubGlobal('confirm', confirm);
});
afterEach(() => vi.unstubAllGlobals());

describe('LivePageDatasetControls', () => {
  it('deletes an unused dataset after one confirmation, once for a double click', async () => {
    const { onChanged, onDeleted } = setup();
    const button = screen.getByRole('button', { name: /pages.dataset.delete/ });
    fireEvent.click(button);
    fireEvent.click(button);
    await waitFor(() => expect(onDeleted).toHaveBeenCalledTimes(1));
    expect(confirm).toHaveBeenCalledTimes(1);
    expect(confirm.mock.calls[0][0]).toBe('pages.dataset.confirmDelete|auto_reviews');
    expect(pagesApi.deleteDataset).toHaveBeenCalledTimes(1);
    expect(pagesApi.deleteDataset).toHaveBeenCalledWith('page-1', 'auto_reviews');
    expect(onChanged).toHaveBeenCalledTimes(1);
  });

  it('cancelled, it deletes nothing', async () => {
    confirm.mockReturnValue(false);
    const { onDeleted } = setup();
    fireEvent.click(screen.getByRole('button', { name: /pages.dataset.delete/ }));
    await waitFor(() => expect(confirm).toHaveBeenCalled());
    expect(pagesApi.deleteDataset).not.toHaveBeenCalled();
    expect(onDeleted).not.toHaveBeenCalled();
  });

  it('shows the references of a dataset in use and forces only on a second confirmation', async () => {
    const refusal = "Dataset 'auto_reviews' is still in use: written by workflow 'Reviews' (wf-1)";
    vi.mocked(pagesApi.deleteDataset).mockRejectedValueOnce(new ApiRequestError(refusal, 'conflict'));
    const { onDeleted } = setup();
    fireEvent.click(screen.getByRole('button', { name: /pages.dataset.delete/ }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(confirm.mock.calls[1][0]).toBe(`pages.dataset.confirmForce|${refusal}`);
    expect(pagesApi.deleteDataset).toHaveBeenLastCalledWith('page-1', 'auto_reviews', true);
  });

  it('declining the forced deletion leaves the dataset', async () => {
    vi.mocked(pagesApi.deleteDataset).mockRejectedValueOnce(new ApiRequestError('in use', 'conflict'));
    confirm.mockReturnValueOnce(true).mockReturnValueOnce(false);
    const { onDeleted } = setup();
    fireEvent.click(screen.getByRole('button', { name: /pages.dataset.delete/ }));
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(2));
    expect(pagesApi.deleteDataset).toHaveBeenCalledTimes(1);
    expect(onDeleted).not.toHaveBeenCalled();
  });

  it('shows any other refusal without forcing', async () => {
    vi.mocked(pagesApi.deleteDataset).mockRejectedValueOnce(new ApiRequestError('Only a human', 'validation'));
    setup();
    fireEvent.click(screen.getByRole('button', { name: /pages.dataset.delete/ }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Only a human');
    expect(pagesApi.deleteDataset).toHaveBeenCalledTimes(1);
  });

  it('empties a dataset with a clear publication', async () => {
    const { onChanged } = setup({ kind: 'snapshot' });
    fireEvent.click(screen.getByRole('button', { name: /pages.dataset.clear/ }));
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
    expect(confirm.mock.calls[0][0]).toBe('pages.dataset.confirmClear|auto_reviews');
    expect(pagesApi.publish).toHaveBeenCalledWith('page-1', expect.objectContaining({
      writes: [expect.objectContaining({ dataset: 'auto_reviews', operation: 'clear' })],
    }));
  });

  it('changes the limits of a time series, an empty age lifting it', async () => {
    const { onChanged } = setup({ max_age_days: 30 });
    fireEvent.change(screen.getByLabelText('pages.dataset.maxPoints'), { target: { value: '20' } });
    fireEvent.change(screen.getByLabelText('pages.dataset.maxAgeDays'), { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: /pages.dataset.saveLimits/ }));
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
    expect(pagesApi.updateDataset).toHaveBeenCalledWith('page-1', 'auto_reviews', { max_points: 20, max_age_days: null });
  });

  it('refuses limits that are not positive whole numbers, without asking', async () => {
    setup();
    fireEvent.change(screen.getByLabelText('pages.dataset.maxPoints'), { target: { value: '0' } });
    fireEvent.click(screen.getByRole('button', { name: /pages.dataset.saveLimits/ }));
    expect(await screen.findByRole('alert')).toHaveTextContent('pages.dataset.limitsInvalid');
    expect(confirm).not.toHaveBeenCalled();
    expect(pagesApi.updateDataset).not.toHaveBeenCalled();
  });

  it('offers limits only on a time series', () => {
    setup({ kind: 'collection' });
    expect(screen.queryByLabelText('pages.dataset.maxPoints')).toBeNull();
  });
});

describe('LivePageDatasetUsageLine', () => {
  it('names the writers, the HTML reference and the buttons', () => {
    const usage: LivePageDatasetUsage = {
      name: 'auto_reviews',
      writers: [{ workflow_id: 'wf-1', workflow_name: 'Reviews', enabled: true }],
      html_referenced: false,
      action_refs: ['move'],
    };
    render(<LivePageDatasetUsageLine dataset={dataset()} usage={usage} />);
    expect(screen.getByText('pages.dataset.writers|Reviews')).toBeInTheDocument();
    expect(screen.getByText('pages.dataset.htmlUnreferenced')).toBeInTheDocument();
    expect(screen.getByText('pages.dataset.buttons|move')).toBeInTheDocument();
  });

  it('says when nothing writes it', () => {
    render(<LivePageDatasetUsageLine dataset={dataset()} usage={{ name: 'x', writers: [], html_referenced: true, action_refs: [] }} />);
    expect(screen.getByText('pages.dataset.noWriter')).toBeInTheDocument();
    expect(screen.getByText('pages.dataset.htmlReferenced')).toBeInTheDocument();
  });
});

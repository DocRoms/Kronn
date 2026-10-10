import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, render, fireEvent, screen, waitFor } from '@testing-library/react';
import { I18nProvider } from '../../lib/I18nContext';

vi.mock('../../lib/api', async () => {
  const { buildApiMock } = await import('../../test/apiMock');
  return buildApiMock();
});

import { pages as pagesApi } from '../../lib/api';
import { LivePageSlugEditor } from '../LivePageSlugEditor';
import { isValidPageSlug } from '../../lib/live-page-slug';
import type { LivePageDetail } from '../../types/generated';

const page = { id: 'page-1', slug: 'mon-suivi-v2', slug_aliases: [] as string[] };
const wrap = (onSaved = vi.fn()) => {
  render(<I18nProvider><LivePageSlugEditor page={page} onSaved={onSaved} /></I18nProvider>);
  return onSaved;
};

beforeEach(() => {
  vi.clearAllMocks();
});

describe('LivePageSlugEditor', () => {
  it('mirrors the backend slug rule', () => {
    expect(isValidPageSlug('suivi-team-front')).toBe(true);
    for (const bad of ['', 'Team', 'a--b', '-a', 'a-', 'a b', '0b8f5c2e-3a7d-4f1e-9c6b-2d4e8a1f7c3b', 'x'.repeat(101)]) {
      expect(isValidPageSlug(bad)).toBe(false);
    }
  });

  it('previews the new link and saves the slug', async () => {
    const updated = { ...page, slug: 'suivi-team-front', slug_aliases: ['mon-suivi-v2'] } as unknown as LivePageDetail;
    vi.mocked(pagesApi.update).mockResolvedValue(updated);
    const onSaved = wrap();
    fireEvent.click(screen.getByRole('button', { name: /mon-suivi-v2/ }));
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: ' suivi-team-front ' } });
    expect(screen.getByTestId('live-page-slug-preview').textContent).toContain('#page/suivi-team-front');
    fireEvent.submit(input.closest('form')!);
    await waitFor(() => expect(onSaved).toHaveBeenCalledWith(updated));
    expect(pagesApi.update).toHaveBeenCalledWith('page-1', { slug: 'suivi-team-front' });
    expect(screen.queryByRole('textbox')).toBeNull();
  });

  it('refuses a bad slug locally and shows a server refusal', async () => {
    vi.mocked(pagesApi.update).mockRejectedValue(new Error('This Page slug is not available; choose another'));
    const onSaved = wrap();
    fireEvent.click(screen.getByRole('button', { name: /mon-suivi-v2/ }));
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'Bad Slug' } });
    fireEvent.submit(input.closest('form')!);
    expect(await screen.findByRole('alert')).toBeTruthy();
    expect(pagesApi.update).not.toHaveBeenCalled();

    fireEvent.change(input, { target: { value: 'taken-slug' } });
    fireEvent.submit(input.closest('form')!);
    expect((await screen.findByRole('alert')).textContent).toContain('not available');
    expect(onSaved).not.toHaveBeenCalled();
  });

  it('sends one update for two submits while a save is pending', async () => {
    vi.mocked(pagesApi.update).mockImplementation(() => new Promise(() => {}));
    wrap();
    fireEvent.click(screen.getByRole('button', { name: /mon-suivi-v2/ }));
    const input = screen.getByRole('textbox');
    fireEvent.change(input, { target: { value: 'after' } });
    const form = input.closest('form')!;
    await act(async () => { fireEvent.submit(form); fireEvent.submit(form); });
    expect(pagesApi.update).toHaveBeenCalledTimes(1);
  });

  it('leaves an unchanged slug without a request, and Escape cancels', () => {
    wrap();
    fireEvent.click(screen.getByRole('button', { name: /mon-suivi-v2/ }));
    fireEvent.submit(screen.getByRole('textbox').closest('form')!);
    expect(pagesApi.update).not.toHaveBeenCalled();
    expect(screen.queryByRole('textbox')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: /mon-suivi-v2/ }));
    fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Escape' });
    expect(screen.queryByRole('textbox')).toBeNull();
  });
});

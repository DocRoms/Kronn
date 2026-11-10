/** KT-1524 — the app-wide notice of connections whose token is refused. */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, act } from '@testing-library/react';

const { mcps } = vi.hoisted(() => ({ mcps: { overview: vi.fn() } }));
vi.mock('../../lib/api', () => ({ mcps }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({
    t: (key: string, ...args: (string | number)[]) =>
      args.length ? `${key} ${args.map(String).join(' ')}` : key,
  }),
}));

import { AuthFailureBanner } from '../AuthFailureBanner';

const config = (id: string, label: string, refused: boolean) => ({
  id,
  label,
  auth_failure: refused
    ? { kind: 'refused', http_status: 401, message: 'Bad credentials', failed_at: '2026-11-10T08:00:00Z' }
    : undefined,
});

describe('AuthFailureBanner', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('lists the refused connections and links each to its card', async () => {
    mcps.overview.mockResolvedValue({
      configs: [config('cfg-1', 'GitHub Euronews', true), config('cfg-2', 'Notion', false), config('cfg-3', 'Adobe', true)],
    });
    render(<AuthFailureBanner />);

    const banner = await screen.findByTestId('auth-failure-banner');
    expect(banner.textContent).toContain('GitHub Euronews, Adobe');
    expect(banner.textContent).not.toContain('Notion');
    expect(screen.getByTestId('auth-failure-fix-cfg-1').getAttribute('href')).toBe('/plugins/cfg-1');
    expect(screen.getByTestId('auth-failure-fix-cfg-3').getAttribute('href')).toBe('/plugins/cfg-3');
  });

  it('says nothing when every connection is fine', async () => {
    mcps.overview.mockResolvedValue({ configs: [config('cfg-2', 'Notion', false)] });
    render(<AuthFailureBanner />);
    await waitFor(() => expect(mcps.overview).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.queryByTestId('auth-failure-banner')).toBeNull();
  });

  it('says nothing when the overview cannot be read', async () => {
    mcps.overview.mockRejectedValue(new Error('offline'));
    render(<AuthFailureBanner />);
    await waitFor(() => expect(mcps.overview).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.queryByTestId('auth-failure-banner')).toBeNull();
  });
});

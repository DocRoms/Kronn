import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageDetail } from '../../types/generated';

const linkRelay = vi.hoisted(() => ({ connect: vi.fn(), dispose: vi.fn() }));

const detail: LivePageDetail = {
  id: 'page-1', project_id: null, title: 'Production health', slug: 'production-health',
  current_revision_id: 'rev-1', data_revision: 2,
  created_at: '2026-08-26T10:00:00Z', updated_at: '2026-08-26T10:00:00Z',
  last_published_at: '2026-08-26T10:00:00Z', pinned: false, archived: false,
  revision: {
    id: 'rev-1', page_id: 'page-1', revision: 1,
    html: '<main><h1>Production health</h1></main>',
    created_by_agent: 'Ollama', created_at: '2026-08-26T10:00:00Z',
  },
  datasets: [],
};

vi.mock('../../lib/api', () => ({ pages: { get: vi.fn() } }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: string[]) => args.length ? `${key}:${args.join(',')}` : key }),
}));
vi.mock('../../lib/live-page-sandbox', async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(),
  createLivePageOpenLinkRelay: vi.fn(() => linkRelay),
}));

import { pages as pagesApi } from '../../lib/api';
import { StandaloneLivePage } from '../StandaloneLivePage';

beforeEach(() => {
  linkRelay.connect.mockClear();
  linkRelay.dispose.mockClear();
  vi.mocked(pagesApi.get).mockResolvedValue(detail);
});

describe('StandaloneLivePage', () => {
  it('renders the requested Page full-screen inside the opaque sandbox', async () => {
    const previousTitle = document.title;
    const view = render(<StandaloneLivePage pageId="page-1" />);

    expect(screen.getByRole('status')).toHaveTextContent('pages.standaloneLoading');
    const frame = await screen.findByTestId('standalone-live-page-frame');
    expect(pagesApi.get).toHaveBeenCalledWith('page-1');
    expect(frame).toHaveAttribute('title', 'Production health');
    expect(frame).toHaveAttribute('sandbox', 'allow-scripts');
    expect(frame).not.toHaveAttribute('allow-same-origin');
    expect(frame.getAttribute('srcdoc')).toContain("connect-src 'none'");
    expect(frame.getAttribute('srcdoc')).toContain('<h1>Production health</h1>');
    await waitFor(() => expect(linkRelay.connect).toHaveBeenCalledWith(
      (frame as HTMLIFrameElement).contentWindow,
    ));
    await waitFor(() => expect(document.title).toBe('Production health · Kronn'));

    view.unmount();
    expect(document.title).toBe(previousTitle);
  });
});

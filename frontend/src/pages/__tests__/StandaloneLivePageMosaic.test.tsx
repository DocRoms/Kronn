import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { LivePageDetail } from '../../types/generated';

const details: Record<string, LivePageDetail> = Object.fromEntries(['page-1', 'page-2', 'page-3'].map((id, index) => [id, {
  id,
  project_id: null,
  title: `Report ${index + 1}`,
  slug: `report-${index + 1}`,
  current_revision_id: `rev-${index + 1}`,
  data_revision: 1,
  created_at: '2026-08-29T10:00:00Z',
  updated_at: '2026-08-29T10:00:00Z',
  last_published_at: null,
  pinned: false,
  archived: false,
  revision: {
    id: `rev-${index + 1}`,
    page_id: id,
    revision: 1,
    html: `<main><h1>Report ${index + 1}</h1></main>`,
    created_by_agent: null,
    created_at: '2026-08-29T10:00:00Z',
  },
  datasets: [],
}]));
const relays = vi.hoisted(() => [] as { connect: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn> }[]);

vi.mock('../../lib/api', () => ({ pages: { get: vi.fn() } }));
vi.mock('../../lib/I18nContext', () => ({
  useT: () => ({ t: (key: string, ...args: string[]) => args.length ? `${key}:${args.join(',')}` : key }),
}));
vi.mock('../../lib/live-page-sandbox', async importOriginal => ({
  ...await importOriginal<Record<string, unknown>>(),
  createLivePageOpenLinkRelay: vi.fn(() => {
    const relay = { connect: vi.fn(), dispose: vi.fn() };
    relays.push(relay);
    return relay;
  }),
}));

import { pages as pagesApi } from '../../lib/api';
import { StandaloneLivePageMosaic } from '../StandaloneLivePageMosaic';

beforeEach(() => {
  relays.length = 0;
  vi.mocked(pagesApi.get).mockImplementation(async pageId => details[pageId]);
});

describe('StandaloneLivePageMosaic', () => {
  it('loads every selected Page in its own opaque sandbox and applies the preset', async () => {
    const previousTitle = document.title;
    const view = render(
      <StandaloneLivePageMosaic
        pageIds={['page-1', 'page-2', 'page-3']}
        layout="three-left"
      />,
    );

    expect(screen.getByTestId('standalone-live-page-mosaic')).toHaveAttribute('data-layout', 'three-left');
    const frames = await screen.findAllByTestId('standalone-live-page-mosaic-frame');
    expect(pagesApi.get).toHaveBeenCalledTimes(3);
    expect(frames.map(frame => frame.getAttribute('title'))).toEqual(['Report 1', 'Report 2', 'Report 3']);
    frames.forEach((frame, index) => {
      expect(frame).toHaveAttribute('sandbox', 'allow-scripts');
      expect(frame).not.toHaveAttribute('allow-same-origin');
      expect(frame.getAttribute('srcdoc')).toContain("connect-src 'none'");
      expect(frame.getAttribute('srcdoc')).toContain(`Report ${index + 1}`);
    });
    await waitFor(() => expect(relays.every(relay => relay.connect.mock.calls.length > 0)).toBe(true));
    expect(document.title).toBe('pages.mosaic.documentTitle · Kronn');

    view.unmount();
    expect(document.title).toBe(previousTitle);
    expect(relays.every(relay => relay.dispose.mock.calls.length === 1)).toBe(true);
  });
});

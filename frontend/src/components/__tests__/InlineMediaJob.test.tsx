/**
 * KT-549 — the media placeholder anchored to its own message.
 *
 * Pending/running/failed states reuse the shared RunStatusCard (self-hydrating
 * scoping is already covered by RunStatusCard.test.tsx); what's specific here
 * is the "hide once succeeded" rule — the generated asset already renders as
 * that same message's attachment, so showing this card too would duplicate
 * the result in the same spot.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { fireEvent, render, screen, cleanup } from '@testing-library/react';

const runsGet = vi.fn();
const contextFileBlob = vi.fn();

vi.mock('../../lib/api', () => ({
  runsApi: { get: (...a: unknown[]) => runsGet(...a), list: vi.fn() },
  discussions: { contextFileBlob: (...a: unknown[]) => contextFileBlob(...a) },
  getApiBase: () => 'http://localhost',
  getAuthToken: () => null,
}));

// Kept not-intersecting for the whole suite: the card must not self-hydrate
// (no fetch, no WebSocket) while off-screen — only the given SharedRun feeds
// the render.
class MockIntersectionObserver {
  callback: (entries: Array<{ isIntersecting: boolean }>) => void;
  constructor(callback: (entries: Array<{ isIntersecting: boolean }>) => void) {
    this.callback = callback;
  }
  observe() {}
  disconnect() {}
}

const { InlineMediaJob } = await import('../InlineMediaJob');
import type { SharedRun } from '../../types/generated';

function mediaRun(overrides: Partial<SharedRun> = {}): SharedRun {
  return {
    id: 'job-1',
    kind: 'media',
    source_id: 'conn-1',
    project_id: null,
    discussion_id: 'disc-1',
    status: 'queued',
    started_at: null,
    finished_at: null,
    duration_ms: null,
    result: { schema_version: 1, modality: 'image', phase: 'submitting' },
    diagnostic: null,
    created_at: '2026-09-01T00:00:00Z',
    updated_at: '2026-09-01T00:00:00Z',
    ...overrides,
  } as SharedRun;
}

let originalObserver: typeof IntersectionObserver | undefined;

beforeEach(() => {
  originalObserver = globalThis.IntersectionObserver;
  (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver =
    MockIntersectionObserver;
  runsGet.mockReset();
  contextFileBlob.mockReset();
  contextFileBlob.mockRejectedValue(new Error('no bytes in this test'));
  // jsdom implements neither, and the preview creates one per asset.
  URL.createObjectURL = vi.fn(() => 'blob:preview');
  URL.revokeObjectURL = vi.fn();
});

afterEach(() => {
  cleanup();
  if (originalObserver) globalThis.IntersectionObserver = originalObserver;
});

describe('InlineMediaJob', () => {
  const renderJob = (run: SharedRun, onOpenAsset = vi.fn()) => render(
    <InlineMediaJob
      discussionId="disc-1"
      messageId="message-1"
      prompt="A small generated landscape"
      run={run}
      onOpenAsset={onOpenAsset}
    />,
  );

  it('renders a queued job without any fabricated progress', () => {
    renderJob(mediaRun({ status: 'queued' }));
    const card = screen.getByTestId('run-status-card');
    expect(card.getAttribute('data-status')).toBe('queued');
    expect(card.getAttribute('data-kind')).toBe('media');
    expect(screen.queryByRole('progressbar')).toBeNull();
    expect(screen.getByText('A small generated landscape')).toBeInTheDocument();
    expect(screen.getByText('A small generated landscape').closest('[data-message-id]'))
      .toHaveAttribute('data-message-id', 'message-1');
  });

  it('renders a running job', () => {
    renderJob(mediaRun({ status: 'running' }));
    expect(screen.getByTestId('run-status-card').getAttribute('data-status')).toBe('running');
  });

  it('renders a clear terminal failure with its diagnostic', () => {
    renderJob(mediaRun({ status: 'failed', diagnostic: 'connection refused' }));
    const card = screen.getByTestId('run-status-card');
    expect(card.getAttribute('data-status')).toBe('failed');
    expect(card.textContent).toContain('connection refused');
  });

  it('stays in the same media bubble on success and opens the exact asset', () => {
    const onOpenAsset = vi.fn();
    renderJob(mediaRun({
      status: 'success',
      result: {
        schema_version: 1,
        modality: 'video',
        phase: 'completed',
        asset_id: 'asset-video-7',
      },
    }), onOpenAsset);
    expect(screen.getByTestId('run-status-card')).toHaveAttribute('data-status', 'success');
    fireEvent.click(screen.getByTestId('media-bubble-open-asset'));
    expect(onOpenAsset).toHaveBeenCalledWith('asset-video-7');
  });

  it('offers one destination only, and it is the media', async () => {
    // The card recomputes an href of its own on every rehydration, so the
    // bubble has to suppress the run link for good — otherwise a second,
    // useless button reappears next to the status a moment after mount.
    const run = mediaRun({
      status: 'success',
      result: { schema_version: 1, modality: 'image', phase: 'completed', asset_id: 'asset-1' },
    });
    runsGet.mockResolvedValue(run);
    renderJob(run);
    expect(screen.queryByRole('link')).toBeNull();
    const open = await screen.findByTestId('media-bubble-open-asset');
    expect(open.textContent).toContain('run.media.open');
  });

  it('shows the produced media inside the bubble', async () => {
    contextFileBlob.mockResolvedValue(new Blob(['x']));
    renderJob(mediaRun({
      status: 'success',
      result: { schema_version: 1, modality: 'image', phase: 'completed', asset_id: 'asset-1' },
    }));

    const preview = await screen.findByTestId('media-bubble-preview');
    expect(contextFileBlob).toHaveBeenCalledWith('disc-1', 'asset-1');
    const image = preview.querySelector('img');
    expect(image).toHaveAttribute('src', 'blob:preview');
    expect(image).toHaveAttribute('alt', 'A small generated landscape');
    expect(preview.querySelector('video')).toBeNull();
  });

  it('plays a generated clip rather than showing it as a still', async () => {
    contextFileBlob.mockResolvedValue(new Blob(['x']));
    renderJob(mediaRun({
      status: 'success',
      result: { schema_version: 1, modality: 'video', phase: 'completed', asset_id: 'asset-2' },
    }));

    const preview = await screen.findByTestId('media-bubble-preview');
    expect(preview.querySelector('video')).toHaveAttribute('controls');
    expect(preview.querySelector('img')).toBeNull();
  });

  it('keeps the result reachable when the preview bytes cannot be fetched', async () => {
    contextFileBlob.mockRejectedValue(new Error('offline'));
    renderJob(mediaRun({
      status: 'success',
      result: { schema_version: 1, modality: 'image', phase: 'completed', asset_id: 'asset-3' },
    }));

    // A preview is a convenience; losing it must not cost the only way to
    // reach the asset.
    expect(await screen.findByTestId('media-bubble-open-asset')).toBeInTheDocument();
    expect(screen.queryByTestId('media-bubble-preview')).toBeNull();
  });

  it('keeps a completed bubble visible while the asset link is still finalising', () => {
    renderJob(mediaRun({ status: 'success' }));
    expect(screen.getByTestId('run-status-card')).toHaveAttribute('data-status', 'success');
    expect(screen.queryByTestId('media-bubble-open-asset')).toBeNull();
  });
});

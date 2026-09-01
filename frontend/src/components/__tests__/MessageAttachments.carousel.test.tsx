/**
 * KT-540 — mixed carousel.
 *
 * The requirement is precise: clicking any asset must let you reach EVERY
 * generated image and video in one sequence. The previous carousel filtered on
 * `isImageFile` AND on an already-loaded blob, so a video could not enter it at
 * all and an image still in flight was silently skipped.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react';

const contextFileBlob = vi.fn();
vi.mock('../../lib/api', () => ({
  discussions: { contextFileBlob: (...a: unknown[]) => contextFileBlob(...a) },
}));

const { MessageAttachments } = await import('../MessageAttachments');
import type { ContextFile } from '../../types/generated';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}|${args.join('|')}` : key;

function file(id: string, filename: string, mime: string): ContextFile {
  return {
    id, discussion_id: 'd1', filename, mime_type: mime,
    original_size: 1024, extracted_size: 0, disk_path: `/tmp/${id}`,
    message_id: null, created_at: '2026-01-01T00:00:00Z',
  } as ContextFile;
}

// The sequence from the requirement: img img img video img img
const FILES = [
  file('i1', 'a.png', 'image/png'),
  file('i2', 'b.png', 'image/png'),
  file('i3', 'c.png', 'image/png'),
  {
    ...file('v1', 'clip.mp4', 'video/mp4'),
    ai_generation: {
      model: 'provider/video-model-v3',
      prompt: 'Slow aerial movement above a green valley',
    },
  },
  file('i4', 'd.png', 'image/png'),
  file('i5', 'e.png', 'image/png'),
];

beforeEach(() => {
  contextFileBlob.mockReset();
  contextFileBlob.mockResolvedValue(new Blob(['x']));
  // Exercise the component's documented non-IntersectionObserver fallback;
  // browser intersection behaviour is covered by the component contract.
  vi.stubGlobal('IntersectionObserver', undefined);
  globalThis.URL.createObjectURL = vi.fn(() => 'blob:stub');
  globalThis.URL.revokeObjectURL = vi.fn();
});
afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe('MessageAttachments — mixed carousel', () => {
  it('counts every image AND video in one sequence', async () => {
    render(<MessageAttachments files={FILES} discussionId="d1" t={t} variant="library" />);

    // Open on the video: it must be a real entry, not a filename chip.
    fireEvent.click(screen.getByTestId('attach-video-thumb'));

    const panel = await screen.findByRole('dialog');
    // 6 media total, video sitting fourth.
    expect(panel.textContent).toContain('4 / 6');
  });

  it('walks from a video to the images on both sides', async () => {
    render(<MessageAttachments files={FILES} discussionId="d1" t={t} variant="library" />);
    fireEvent.click(screen.getByTestId('attach-video-thumb'));
    const panel = await screen.findByRole('dialog');
    expect(panel.textContent).toContain('4 / 6');

    fireEvent.keyDown(document, { key: 'ArrowRight' });
    await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain('5 / 6'));

    fireEvent.keyDown(document, { key: 'ArrowLeft' });
    fireEvent.keyDown(document, { key: 'ArrowLeft' });
    await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain('3 / 6'));
  });

  it('wraps around, so the sequence has no dead end', async () => {
    render(<MessageAttachments files={FILES} discussionId="d1" t={t} variant="library" />);
    fireEvent.click(screen.getByTestId('attach-video-thumb'));
    await screen.findByRole('dialog');

    // 4 → 5 → 6 → 1
    for (const expected of ['5 / 6', '6 / 6', '1 / 6']) {
      fireEvent.keyDown(document, { key: 'ArrowRight' });
      await waitFor(() => expect(screen.getByRole('dialog').textContent).toContain(expected));
    }
  });

  it('plays the video in the carousel rather than offering a download chip', async () => {
    render(<MessageAttachments files={FILES} discussionId="d1" t={t} variant="library" />);
    fireEvent.click(screen.getByTestId('attach-video-thumb'));

    // The player appears once the selected clip's bytes arrive.
    await waitFor(() => expect(screen.getByTestId('media-player-video')).toBeTruthy());
    const video = screen.getByTestId('media-player-video') as HTMLVideoElement;
    expect(video.autoplay).toBe(true);
    expect(video.muted).toBe(true);
    expect(video.getAttribute('preload')).toBe('metadata');
  });

  it('shows the generated-video model and prompt directly below its player', async () => {
    render(<MessageAttachments files={FILES} discussionId="d1" t={t} variant="library" />);
    fireEvent.click(screen.getByTestId('attach-video-thumb'));

    await waitFor(() => expect(screen.getByTestId('media-player-video')).toBeTruthy());
    const details = screen.getByTestId('ai-generation-details');
    expect(details).toHaveTextContent('provider/video-model-v3');
    expect(details).toHaveTextContent('Slow aerial movement above a green valley');
  });

  it('loads a real thumbnail for a visible library clip and reuses its blob in the player', async () => {
    render(<MessageAttachments files={FILES} discussionId="d1" t={t} variant="library" />);
    // jsdom has no IntersectionObserver, so visible-card loading falls back to
    // immediate preparation. A browser does the same only near the viewport.
    await waitFor(() =>
      expect(contextFileBlob.mock.calls.map(c => c[1])).toContain('v1'),
    );
    expect(await screen.findByTestId('attach-video-poster')).toHaveAttribute('src', 'blob:stub');
    const videoFetchesBeforeOpen = contextFileBlob.mock.calls.filter(c => c[1] === 'v1').length;

    fireEvent.click(screen.getByTestId('attach-video-thumb'));
    await waitFor(() => expect(screen.getByTestId('media-player-video')).toBeTruthy());
    expect(contextFileBlob.mock.calls.filter(c => c[1] === 'v1')).toHaveLength(videoFetchesBeforeOpen);
  });

  it('keeps the video URL valid when the viewer is closed and reopened', async () => {
    const { unmount } = render(
      <MessageAttachments files={FILES} discussionId="d1" t={t} variant="library" />,
    );
    await screen.findByTestId('attach-video-poster');

    fireEvent.click(screen.getByTestId('attach-video-thumb'));
    await waitFor(() => expect(screen.getByTestId('media-player-video')).toHaveAttribute('src', 'blob:stub'));
    fireEvent.click(screen.getByRole('button', { name: 'disc.attachmentClose' }));
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('attach-video-thumb'));
    await waitFor(() => expect(screen.getByTestId('media-player-video')).toHaveAttribute('src', 'blob:stub'));
    expect(contextFileBlob.mock.calls.filter(c => c[1] === 'v1')).toHaveLength(1);
    expect(URL.revokeObjectURL).not.toHaveBeenCalled();

    unmount();
    await waitFor(() => expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:stub'));
  });

  it('leaves a non-media document out of the carousel', async () => {
    const withDoc = [...FILES, file('p1', 'report.pdf', 'application/pdf')];
    render(<MessageAttachments files={withDoc} discussionId="d1" t={t} variant="library" />);
    fireEvent.click(screen.getByTestId('attach-video-thumb'));
    const panel = await screen.findByRole('dialog');
    // Still 6: the PDF is listed as an asset but is not walkable.
    expect(panel.textContent).toContain('4 / 6');
    expect(screen.getAllByTestId('attach-chip').length).toBeGreaterThan(0);
  });
});

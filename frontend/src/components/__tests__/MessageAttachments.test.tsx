import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup, fireEvent, within } from '@testing-library/react';
import type { ContextFile } from '../../types/generated';

// 0.8.8 — MessageAttachments renders files pinned to a message: image
// thumbnails fetched as auth'd blobs, filename chips for everything else.
const { discussionsApi } = vi.hoisted(() => ({
  discussionsApi: { contextFileBlob: vi.fn() },
}));

vi.mock('../../lib/api', () => ({ discussions: discussionsApi }));

import { MessageAttachments } from '../MessageAttachments';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}:${args.join(',')}` : key;

function mkFile(over: Partial<ContextFile> = {}): ContextFile {
  return {
    id: 'cf1',
    discussion_id: 'd1',
    filename: 'shot.png',
    mime_type: 'image/png',
    original_size: 2048,
    extracted_size: 0,
    disk_path: '/tmp/shot.png',
    message_id: 'm1',
    created_at: '2026-06-17T10:00:00Z',
    ...over,
  };
}

describe('MessageAttachments', () => {
  beforeEach(() => {
    cleanup();
    vi.clearAllMocks();
    // jsdom has no object-URL impl.
    globalThis.URL.createObjectURL = vi.fn(() => 'blob:fake-url');
    globalThis.URL.revokeObjectURL = vi.fn();
  });
  afterEach(() => cleanup());

  it('renders nothing when there are no files', () => {
    const { container } = render(<MessageAttachments files={[]} discussionId="d1" t={t} />);
    expect(container.firstChild).toBeNull();
  });

  it('fetches an image as a blob and renders it as a thumbnail', async () => {
    discussionsApi.contextFileBlob.mockResolvedValue(new Blob(['x'], { type: 'image/png' }));
    render(<MessageAttachments files={[mkFile()]} discussionId="d1" t={t} />);

    await waitFor(() => {
      const img = screen.getByRole('img');
      expect(img).toHaveAttribute('src', 'blob:fake-url');
      expect(img).toHaveAttribute('alt', 'shot.png');
      expect(img.closest('button')).toHaveAttribute('aria-label', 'disc.attachmentImage:shot.png');
    });
    expect(discussionsApi.contextFileBlob).toHaveBeenCalledWith('d1', 'cf1');
  });

  it('renders a filename chip (no fetch) for a non-image file', async () => {
    const txt = mkFile({ id: 'cf2', filename: 'notes.txt', mime_type: 'text/plain', disk_path: null });
    render(<MessageAttachments files={[txt]} discussionId="d1" t={t} />);

    expect(screen.getByTestId('attach-chip')).toHaveTextContent('notes.txt');
    expect(screen.queryByRole('img')).toBeNull();
    // No disk_path → no byte fetch.
    expect(discussionsApi.contextFileBlob).not.toHaveBeenCalled();
  });

  it('does not mistake a disk-backed non-image attachment for a thumbnail', () => {
    const csv = mkFile({
      id: 'cf-csv',
      filename: 'metrics.csv',
      mime_type: 'text/csv',
      disk_path: '/tmp/metrics.csv',
    });
    render(<MessageAttachments files={[csv]} discussionId="d1" t={t} />);

    expect(screen.getByTestId('attach-chip')).toHaveTextContent('metrics.csv');
    expect(screen.queryByRole('img')).toBeNull();
    expect(discussionsApi.contextFileBlob).not.toHaveBeenCalled();
  });

  it('falls back to a chip when the image bytes fail to load', async () => {
    discussionsApi.contextFileBlob.mockRejectedValue(new Error('403'));
    render(<MessageAttachments files={[mkFile({ filename: 'broken.png' })]} discussionId="d1" t={t} />);

    await waitFor(() => {
      expect(screen.getByTestId('attach-chip')).toHaveTextContent('broken.png');
    });
    expect(screen.queryByRole('img')).toBeNull();
  });

  it('renders one node per file', async () => {
    discussionsApi.contextFileBlob.mockResolvedValue(new Blob(['x'], { type: 'image/png' }));
    const files = [
      mkFile({ id: 'a', filename: 'a.png' }),
      mkFile({ id: 'b', filename: 'b.txt', mime_type: 'text/plain', disk_path: null }),
    ];
    render(<MessageAttachments files={files} discussionId="d1" t={t} />);

    await waitFor(() => expect(screen.getByRole('img')).toBeInTheDocument());
    expect(screen.getByTestId('msg-attachments').children).toHaveLength(2);
  });

  it('opens an in-app gallery and navigates between all attached images', async () => {
    discussionsApi.contextFileBlob.mockResolvedValue(new Blob(['x'], { type: 'image/png' }));
    const files = [
      mkFile({ id: 'a', filename: 'a.png' }),
      mkFile({ id: 'b', filename: 'b.png' }),
      mkFile({ id: 'c', filename: 'c.png' }),
    ];
    render(<MessageAttachments files={files} discussionId="d1" t={t} />);

    const secondThumb = await screen.findByRole('button', { name: 'disc.attachmentImage:b.png' });
    await waitFor(() => expect(secondThumb).not.toBeDisabled());
    fireEvent.click(secondThumb);

    const dialog = screen.getByRole('dialog', { name: 'disc.attachmentGallery' });
    expect(dialog).toHaveTextContent('2 / 3');
    expect(within(dialog).getByRole('img', { name: 'b.png' })).toBeInTheDocument();
    const external = screen.getByRole('link', { name: 'disc.attachmentOpenNewTab' });
    expect(external).toHaveAttribute('href', 'blob:fake-url');
    expect(external).toHaveAttribute('target', '_blank');

    fireEvent.click(screen.getByRole('button', { name: 'disc.media.carouselNext' }));
    expect(dialog).toHaveTextContent('3 / 3');
    expect(within(dialog).getByRole('img', { name: 'c.png' })).toBeInTheDocument();

    fireEvent.keyDown(document, { key: 'ArrowRight' });
    expect(dialog).toHaveTextContent('1 / 3');
    expect(within(dialog).getByRole('img', { name: 'a.png' })).toBeInTheDocument();

    fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog', { name: 'disc.attachmentGallery' })).toBeNull();
  });
  it('browses every image and clip of the discussion, not just this message', async () => {
    // The grid under one message holds a single image; the discussion also
    // holds an earlier clip and a later image pinned to other messages.
    discussionsApi.contextFileBlob.mockResolvedValue(new Blob(['x'], { type: 'image/png' }));
    const own = mkFile({ id: 'b', filename: 'own.png', message_id: 'm2' });
    const wholeDiscussion = [
      mkFile({
        id: 'a',
        filename: 'clip.mp4',
        mime_type: 'video/mp4',
        message_id: 'm1',
        created_at: '2026-06-17T09:00:00Z',
      }),
      own,
      mkFile({
        id: 'c',
        filename: 'later.png',
        message_id: 'm3',
        created_at: '2026-06-17T11:00:00Z',
      }),
    ];
    render(
      <MessageAttachments
        files={[own]}
        carouselScope={wholeDiscussion}
        discussionId="d1"
        t={t}
      />,
    );

    // Only this message's own thumbnail is on screen...
    expect(screen.queryByRole('button', { name: 'disc.attachmentImage:later.png' })).toBeNull();
    const thumb = await screen.findByRole('button', { name: 'disc.attachmentImage:own.png' });
    await waitFor(() => expect(thumb).not.toBeDisabled());
    fireEvent.click(thumb);

    // ...yet the carousel it opens walks the whole discussion, mixing the
    // video in with the images.
    const dialog = screen.getByRole('dialog', { name: 'disc.attachmentGallery' });
    expect(dialog).toHaveTextContent('2 / 3');
    fireEvent.click(screen.getByRole('button', { name: 'disc.media.carouselNext' }));
    expect(dialog).toHaveTextContent('3 / 3');
    expect(within(dialog).getByRole('img', { name: 'later.png' })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: 'ArrowRight' });
    expect(dialog).toHaveTextContent('1 / 3');
    // The clip's bytes are fetched on selection, so the player appears once
    // the blob lands — mixing a video into the same sequence as the images.
    await waitFor(() =>
      expect(within(dialog).getByTestId('media-player-video')).toHaveAttribute(
        'aria-label',
        'disc.media.playerLabel:clip.mp4',
      ),
    );
  });

  it('still opens a thumbnail the scope does not list', async () => {
    // A filtered or paginated library must not open on nothing.
    discussionsApi.contextFileBlob.mockResolvedValue(new Blob(['x'], { type: 'image/png' }));
    const shown = mkFile({ id: 'z', filename: 'unlisted.png' });
    render(
      <MessageAttachments
        files={[shown]}
        carouselScope={[mkFile({ id: 'a', filename: 'other.png' })]}
        discussionId="d1"
        t={t}
      />,
    );
    const thumb = await screen.findByRole('button', { name: 'disc.attachmentImage:unlisted.png' });
    await waitFor(() => expect(thumb).not.toBeDisabled());
    fireEvent.click(thumb);
    const dialog = screen.getByRole('dialog', { name: 'disc.attachmentGallery' });
    expect(within(dialog).getByRole('img', { name: 'unlisted.png' })).toBeInTheDocument();
    expect(dialog).toHaveTextContent('2 / 2');
  });
});

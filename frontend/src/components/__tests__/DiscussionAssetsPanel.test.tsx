import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ContextFile } from '../../types/generated';

const { discussionsApi, triggerDownload } = vi.hoisted(() => ({
  discussionsApi: { contextFileBlob: vi.fn() },
  triggerDownload: vi.fn(),
}));

vi.mock('../../lib/api', () => ({ discussions: discussionsApi }));
vi.mock('../../lib/downloadBlob', () => ({ triggerDownload }));

import { DiscussionAssetsPanel } from '../DiscussionAssetsPanel';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}:${args.join(',')}` : key;

function file(index: number, overrides: Partial<ContextFile> = {}): ContextFile {
  return {
    id: `file-${index}`,
    discussion_id: 'disc-1',
    filename: `asset-${index}.txt`,
    mime_type: 'text/plain',
    original_size: 512,
    extracted_size: 512,
    disk_path: null,
    message_id: `message-${index}`,
    created_at: `2026-08-${String((index % 28) + 1).padStart(2, '0')}T10:00:00Z`,
    ...overrides,
  };
}

describe('DiscussionAssetsPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    globalThis.URL.createObjectURL = vi.fn(({ type }: Blob) => `blob:${type}`);
    globalThis.URL.revokeObjectURL = vi.fn();
    discussionsApi.contextFileBlob.mockResolvedValue(new Blob(['image'], { type: 'image/png' }));
  });

  it('searches and filters every discussion asset without scanning messages', async () => {
    const files = [
      file(1, { filename: 'dashboard.png', mime_type: 'image/png', disk_path: '/tmp/dashboard.png' }),
      file(2, { filename: 'metrics.csv', mime_type: 'text/csv' }),
      file(3, { filename: 'draft.pdf', mime_type: 'application/pdf', message_id: null }),
    ];
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={files}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        t={t}
      />,
    );

    expect(screen.getByRole('complementary', { name: 'disc.assets.title' })).toBeInTheDocument();
    expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(3);

    fireEvent.change(screen.getByRole('searchbox', { name: 'disc.assets.search' }), {
      target: { value: 'metrics' },
    });
    expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(1);
    expect(screen.getAllByText('metrics.csv')).toHaveLength(2);

    fireEvent.change(screen.getByRole('searchbox', { name: 'disc.assets.search' }), {
      target: { value: '' },
    });
    fireEvent.click(screen.getByRole('button', { name: /disc\.assets\.filterImages/ }));
    await waitFor(() => expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(1));
    expect(screen.getAllByText('dashboard.png')).toHaveLength(1);

    fireEvent.click(screen.getByRole('button', { name: /disc\.assets\.filterPending/ }));
    expect(screen.getAllByText('draft.pdf')).toHaveLength(2);
    expect(screen.getByText('disc.assets.pending')).toBeInTheDocument();
  });

  it('jumps from an asset to its exact source message', () => {
    const onNavigateMessage = vi.fn();
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[file(7, { filename: 'evidence.txt', message_id: 'message-source' })]}
        onClose={vi.fn()}
        onNavigateMessage={onNavigateMessage}
        t={t}
      />,
    );

    fireEvent.click(screen.getByRole('button', {
      name: 'disc.assets.goToMessageFor:evidence.txt',
    }));
    expect(onNavigateMessage).toHaveBeenCalledWith('message-source');
  });

  it('downloads a disk-backed asset on demand', async () => {
    const blob = new Blob(['csv'], { type: 'text/csv' });
    discussionsApi.contextFileBlob.mockResolvedValueOnce(blob);
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[file(8, { filename: 'export.csv', mime_type: 'text/csv', disk_path: '/tmp/export.csv' })]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        t={t}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: 'disc.assets.downloadFor:export.csv' }));
    await waitFor(() => expect(triggerDownload).toHaveBeenCalledWith('export.csv', blob));
    expect(discussionsApi.contextFileBlob).toHaveBeenCalledWith('disc-1', 'file-8');
  });

  it('loads large histories in bounded pages of forty assets', () => {
    const files = Array.from({ length: 45 }, (_, index) => file(index));
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={files}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        t={t}
      />,
    );

    expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(40);
    const loadMore = screen.getByRole('button', { name: 'disc.assets.loadMore:5' });
    fireEvent.click(loadMore);
    expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(45);
    expect(screen.queryByRole('button', { name: /disc\.assets\.loadMore/ })).toBeNull();
  });

  it('keeps the existing in-app carousel for images opened from the inventory', async () => {
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[
          file(1, { filename: 'one.png', mime_type: 'image/png', disk_path: '/tmp/one.png' }),
          file(2, { filename: 'two.png', mime_type: 'image/png', disk_path: '/tmp/two.png' }),
        ]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        t={t}
      />,
    );

    const first = await screen.findByRole('button', { name: 'disc.attachmentImage:one.png' });
    await waitFor(() => expect(first).not.toBeDisabled());
    fireEvent.click(first);
    const dialog = screen.getByRole('dialog', { name: 'disc.attachmentGallery' });
    expect(dialog).toHaveTextContent('2 / 2');
    expect(within(dialog).getByRole('img', { name: 'one.png' })).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole('button', { name: 'disc.media.carouselNext' }));
    expect(dialog).toHaveTextContent('1 / 2');
    expect(within(dialog).getByRole('img', { name: 'two.png' })).toBeInTheDocument();
  });
  it('reaches images and clips filtered out of the grid', async () => {
    // The "images" filter hides the clip from the inventory, but the carousel
    // is a viewer for everything the discussion generated: one sequence,
    // images and videos together.
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[
          file(3, { filename: 'clip.mp4', mime_type: 'video/mp4', disk_path: '/tmp/clip.mp4' }),
          file(2, { filename: 'shot.png', mime_type: 'image/png', disk_path: '/tmp/shot.png' }),
          file(1, { filename: 'notes.csv', mime_type: 'text/csv' }),
        ]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        t={t}
      />,
    );

    fireEvent.click(screen.getByRole('button', { name: /disc\.assets\.filterImages/ }));
    expect(screen.queryByRole('button', { name: 'disc.media.playerLabel:clip.mp4' })).toBeNull();

    const thumb = await screen.findByRole('button', { name: 'disc.attachmentImage:shot.png' });
    await waitFor(() => expect(thumb).not.toBeDisabled());
    fireEvent.click(thumb);

    const dialog = screen.getByRole('dialog', { name: 'disc.attachmentGallery' });
    // Two media in the discussion, the clip included, even under the filter.
    expect(dialog).toHaveTextContent('2 / 2');
    fireEvent.click(within(dialog).getByRole('button', { name: 'disc.media.carouselNext' }));
    await waitFor(() =>
      expect(within(dialog).getByTestId('media-player-video')).toHaveAttribute(
        'aria-label',
        'disc.media.playerLabel:clip.mp4',
      ),
    );
  });
  it('counts a generated clip as a video, not as a plain file', async () => {
    // Before the media work, "Fichiers" held the clip next to a CSV: the
    // filters only knew about images, so a generated video read as a document.
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[
          file(3, { filename: 'clip.mp4', mime_type: 'video/mp4', disk_path: '/tmp/clip.mp4' }),
          file(2, { filename: 'shot.png', mime_type: 'image/png', disk_path: '/tmp/shot.png' }),
          file(1, { filename: 'notes.csv', mime_type: 'text/csv' }),
        ]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        t={t}
      />,
    );

    const countFor = (label: RegExp) =>
      screen.getByRole('button', { name: label }).querySelector('.disc-assets-filter-count')
        ?.textContent;
    expect(countFor(/disc\.assets\.filterVideos/)).toBe('1');
    expect(countFor(/disc\.assets\.filterImages/)).toBe('1');
    // The CSV, and only the CSV.
    expect(countFor(/disc\.assets\.filterFiles/)).toBe('1');

    fireEvent.click(screen.getByRole('button', { name: /disc\.assets\.filterVideos/ }));
    await waitFor(() => expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(1));
    expect(screen.getByText('clip.mp4')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /disc\.assets\.filterFiles/ }));
    await waitFor(() => expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(1));
    // Neither media is left in the documents bucket.
    expect(screen.queryByText('clip.mp4')).toBeNull();
    expect(screen.queryByText('shot.png')).toBeNull();
  });
});

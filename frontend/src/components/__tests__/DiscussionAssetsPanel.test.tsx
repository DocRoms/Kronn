import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ContextFile } from '../../types/generated';
import { LastFrameError } from '../../lib/lastFrame';
import type * as LastFrameModule from '../../lib/lastFrame';

const { discussionsApi, mediaApi, triggerDownload, extractLastFrame } = vi.hoisted(() => ({
  discussionsApi: { contextFileBlob: vi.fn(), deleteContextFile: vi.fn(), uploadContextFile: vi.fn() },
  mediaApi: { capabilities: vi.fn(), estimate: vi.fn(), generate: vi.fn() },
  triggerDownload: vi.fn(),
  extractLastFrame: vi.fn(),
}));

vi.mock('../../lib/api', () => ({ discussions: discussionsApi, media: mediaApi }));
vi.mock('../../lib/downloadBlob', () => ({ triggerDownload }));
// The decode itself is covered against a fake element in lastFrame.test.ts;
// what this file owns is the trip from the click to the discussion's files.
vi.mock('../../lib/lastFrame', async () => {
  const actual = await vi.importActual<typeof LastFrameModule>('../../lib/lastFrame');
  return { ...actual, extractLastFrame };
});

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
    ai_generation: null,
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
    discussionsApi.deleteContextFile.mockResolvedValue(undefined);
    mediaApi.capabilities.mockResolvedValue({ model: 'image-model', capabilities: { max_input_references: 1 } });
    mediaApi.estimate.mockResolvedValue({ model: 'image-model', estimated_usd: null, samples: 0 });
    extractLastFrame.mockResolvedValue({
      blob: new Blob(['png'], { type: 'image/png' }),
      width: 640,
      height: 640,
    });
  });

  /** A clip of the discussion, as the viewer sees it. */
  function clip(overrides: Partial<ContextFile> = {}): ContextFile {
    return file(1, {
      filename: 'seedance-clip.mp4',
      mime_type: 'video/mp4',
      disk_path: '/tmp/seedance-clip.mp4',
      ...overrides,
    });
  }

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

  it('reveals the image form with the current carousel asset after generation handoff', async () => {
    const connections = [{
      id: 'conn-1', display_name: 'OpenRouter', mention_alias: '@openrouter',
      endpoint: 'https://openrouter.ai/api/v1', origin_preset: 'open_router', has_credential: true,
      economy_model: null, default_model: null, reasoning_model: null,
      image_model: 'image-model', video_model: null, media_endpoint: null,
      created_at: '2026-08-31T10:00:00Z', updated_at: '2026-08-31T10:00:00Z',
    }] as never;
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[file(1, { filename: 'source.png', mime_type: 'image/png', disk_path: '/tmp/source.png' })]}
        connections={connections}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        t={t}
      />,
    );

    fireEvent.click(await screen.findByRole('button', { name: 'disc.attachmentImage:source.png' }));
    fireEvent.click(await screen.findByTestId('attachment-generate-image'));
    expect(screen.queryByRole('dialog', { name: 'disc.attachmentGallery' })).toBeNull();
    const form = await screen.findByTestId('media-generate-form');
    expect(screen.getByTestId('media-slot-conn-1:image')).toHaveAttribute('aria-checked', 'true');
    expect(await screen.findByTestId('media-reference-picker')).toHaveTextContent('source.png');
    expect(form).toHaveFocus();
    expect(mediaApi.generate).not.toHaveBeenCalled();
  });
  it('opens the exact requested asset once and allows an explicit reopen', async () => {
    discussionsApi.contextFileBlob.mockResolvedValue(new Blob(['video'], { type: 'video/mp4' }));
    const files = [
      file(1, { filename: 'other.png', mime_type: 'image/png', disk_path: '/tmp/other.png' }),
      file(2, { filename: 'target.mp4', mime_type: 'video/mp4', disk_path: '/tmp/target.mp4' }),
    ];
    const baseProps = {
      discussionId: 'disc-1',
      files,
      onClose: vi.fn(),
      onNavigateMessage: vi.fn(),
      t,
    };
    const { rerender } = render(
      <DiscussionAssetsPanel
        {...baseProps}
        openAssetRequest={{ assetId: 'file-2', nonce: 1 }}
      />,
    );

    const video = await screen.findByTestId('media-player-video');
    expect(video).toHaveAttribute('aria-label', 'disc.media.playerLabel:target.mp4');
    expect((video as HTMLVideoElement).autoplay).toBe(true);

    fireEvent.click(screen.getByRole('button', { name: 'disc.attachmentClose' }));
    expect(screen.queryByRole('dialog', { name: 'disc.attachmentGallery' })).toBeNull();

    // An ordinary rerender must not reopen a viewer the human just closed.
    rerender(
      <DiscussionAssetsPanel
        {...baseProps}
        openAssetRequest={{ assetId: 'file-2', nonce: 1 }}
      />,
    );
    expect(screen.queryByRole('dialog', { name: 'disc.attachmentGallery' })).toBeNull();

    // A fresh click on the same bubble carries a new nonce and deliberately
    // opens that same asset again.
    rerender(
      <DiscussionAssetsPanel
        {...baseProps}
        openAssetRequest={{ assetId: 'file-2', nonce: 2 }}
      />,
    );
    expect(await screen.findByTestId('media-player-video')).toHaveAttribute(
      'aria-label',
      'disc.media.playerLabel:target.mp4',
    );
  });
  it('scrolls the grid to a requested asset that sits past the first page', async () => {
    // 45 assets, so the target is on the second page. The open request clears
    // the search, and that reset used to snap the grid back to page one — the
    // viewer opened on the right asset but the grid behind it never reached it.
    const files = Array.from({ length: 45 }, (_, index) => file(index + 1, {
      filename: `shot-${index + 1}.png`,
      mime_type: 'image/png',
      disk_path: `/tmp/shot-${index + 1}.png`,
      created_at: `2026-08-01T10:${String(59 - index).padStart(2, '0')}:00Z`,
    }));
    const baseProps = {
      discussionId: 'disc-1',
      files,
      onClose: vi.fn(),
      onNavigateMessage: vi.fn(),
      t,
    };
    const { rerender } = render(<DiscussionAssetsPanel {...baseProps} />);
    expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(40);

    fireEvent.change(screen.getByLabelText('disc.assets.search'), { target: { value: 'shot-4' } });
    rerender(
      <DiscussionAssetsPanel
        {...baseProps}
        openAssetRequest={{ assetId: 'file-45', nonce: 1 }}
      />,
    );

    await waitFor(() => expect(screen.getAllByTestId('discussion-asset-card')).toHaveLength(45));
    expect(screen.getAllByTitle('shot-45.png')[0]).toBeInTheDocument();
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
  it('offers the generation entry even before any media model is configured', async () => {
    // Hiding the entry made the feature undiscoverable: nothing told the
    // operator a media slot has to be filled first, so nobody looked.
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[file(1)]}
        connections={[]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        t={t}
      />,
    );

    expect(screen.getByTestId('assets-generate-toggle')).toBeInTheDocument();
    // And the reason is on screen without a click.
    expect(screen.getByTestId('assets-generate-hint')).toHaveTextContent('disc.media.noSlot');
  });

  /// KT-554 — deleting an asset removes bytes from disk, so it takes two steps
  /// and never happens on screen before the server confirmed it.
  it('deletes an asset only after a confirmation, and closes the viewer', async () => {
    const onAssetDeleted = vi.fn();
    const files = [
      file(1, { filename: 'dashboard.png', mime_type: 'image/png', disk_path: '/tmp/dashboard.png' }),
      file(2, { filename: 'other.png', mime_type: 'image/png', disk_path: '/tmp/other.png' }),
    ];
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={files}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        onAssetDeleted={onAssetDeleted}
        openAssetRequest={{ assetId: 'file-1', nonce: 1 }}
        t={t}
      />,
    );

    const viewer = await screen.findByRole('dialog');

    // First click only arms it: one click away from the close button must not
    // destroy a file.
    fireEvent.click(within(viewer).getByTestId('attachment-delete'));
    expect(discussionsApi.deleteContextFile).not.toHaveBeenCalled();

    fireEvent.click(within(viewer).getByTestId('attachment-delete-confirm'));
    await waitFor(() => expect(discussionsApi.deleteContextFile).toHaveBeenCalledWith('disc-1', 'file-1'));
    expect(onAssetDeleted).toHaveBeenCalledWith('file-1');
    // Closed rather than advanced: landing silently on the neighbouring image
    // would read as having deleted the wrong one.
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
  });

  it('keeps an asset on screen when the server refuses to delete it', async () => {
    discussionsApi.deleteContextFile.mockRejectedValue(new Error('file is still in use'));
    const onAssetDeleted = vi.fn();
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[file(1, { filename: 'dashboard.png', mime_type: 'image/png', disk_path: '/tmp/dashboard.png' })]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        onAssetDeleted={onAssetDeleted}
        openAssetRequest={{ assetId: 'file-1', nonce: 1 }}
        t={t}
      />,
    );

    const viewer = await screen.findByRole('dialog');
    fireEvent.click(within(viewer).getByTestId('attachment-delete'));
    fireEvent.click(within(viewer).getByTestId('attachment-delete-confirm'));

    expect(await screen.findByTestId('attachment-delete-error')).toHaveTextContent('file is still in use');
    // Nothing was removed anywhere: the asset must never vanish from the UI
    // without having been deleted on the server.
    expect(onAssetDeleted).not.toHaveBeenCalled();
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });


  it('keeps a clip\'s last frame as a file of the discussion', async () => {
    // The whole point of KT-556: the picture must land in the room's own
    // inventory, so the launcher offers it as a starting image with no
    // download and no re-upload in between.
    const extracted = file(2, { filename: 'seedance-clip-last-frame.png', mime_type: 'image/png', disk_path: '/tmp/frame.png' });
    discussionsApi.uploadContextFile.mockResolvedValue({ file: extracted });
    const onAssetExtracted = vi.fn();
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[clip()]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        onAssetExtracted={onAssetExtracted}
        openAssetRequest={{ assetId: 'file-1', nonce: 1 }}
        t={t}
      />,
    );

    const viewer = await screen.findByRole('dialog');
    fireEvent.click(await within(viewer).findByTestId('attachment-last-frame'));

    await waitFor(() => expect(discussionsApi.uploadContextFile).toHaveBeenCalledTimes(1));
    const [discussionId, uploaded, extractedFrom] = discussionsApi.uploadContextFile.mock.calls[0];
    expect(discussionId).toBe('disc-1');
    // Named as coming from this clip: the server then gives it its OWN
    // message. Without this it stayed a pending attachment, waiting to be
    // pinned to whatever the user sent next — and deleting THAT message took
    // the picture with it.
    expect(extractedFrom).toBe('file-1');
    // Named after the clip it came from, and a PNG: the launcher filters its
    // starting pictures on the MIME type.
    expect(uploaded.name).toBe('seedance-clip-last-frame.png');
    expect(uploaded.type).toBe('image/png');
    expect(onAssetExtracted).toHaveBeenCalledWith(extracted);
  });

  it('shows where an extracted picture came from, and opens the clip', async () => {
    const clipFile = clip();
    const extracted = file(2, {
      filename: 'seedance-clip-last-frame.png',
      mime_type: 'image/png',
      disk_path: '/tmp/frame.png',
      extracted_from_asset_id: clipFile.id,
    });
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[clipFile, extracted]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        openAssetRequest={{ assetId: extracted.id, nonce: 1 }}
        t={t}
      />,
    );

    const viewer = await screen.findByRole('dialog');
    const provenance = await within(viewer).findByTestId('extracted-from-details');
    expect(provenance).toHaveTextContent('disc.assets.extractedFromExplained');
    // Never the AI badge: that one is an attestation, and nothing was
    // generated — a picture was cut out of a clip.
    expect(within(viewer).queryByTestId('ai-generation-details')).toBeNull();

    // The clip is one click away, inside the same viewer.
    fireEvent.click(within(viewer).getByTestId('extracted-from-open-source'));
    await waitFor(() =>
      expect(screen.getByRole('dialog')).toHaveAttribute('data-asset-id', clipFile.id));
  });

  it('says nothing was decoded instead of attaching a black picture', async () => {
    // The central trap: an extractor without this check would attach the 9th
    // frame out of 97, or a blank rectangle — crisp, exportable and wrong.
    extractLastFrame.mockRejectedValue(new LastFrameError('decode'));
    const onAssetExtracted = vi.fn();
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[clip()]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        onAssetExtracted={onAssetExtracted}
        openAssetRequest={{ assetId: 'file-1', nonce: 1 }}
        t={t}
      />,
    );

    const viewer = await screen.findByRole('dialog');
    fireEvent.click(await within(viewer).findByTestId('attachment-last-frame'));

    expect(await screen.findByTestId('attachment-last-frame-error'))
      .toHaveTextContent('disc.media.lastFrame.error.decode');
    expect(discussionsApi.uploadContextFile).not.toHaveBeenCalled();
    expect(onAssetExtracted).not.toHaveBeenCalled();
  });

  it('offers no extraction on an image, nor on a surface that cannot show the result', async () => {
    const { rerender } = render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[file(1, { filename: 'dashboard.png', mime_type: 'image/png', disk_path: '/tmp/dashboard.png' })]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        onAssetExtracted={vi.fn()}
        openAssetRequest={{ assetId: 'file-1', nonce: 1 }}
        t={t}
      />,
    );
    expect(within(await screen.findByRole('dialog')).queryByTestId('attachment-last-frame')).toBeNull();

    rerender(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[clip()]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        openAssetRequest={{ assetId: 'file-1', nonce: 2 }}
        t={t}
      />,
    );
    const viewer = await screen.findByRole('dialog');
    // Waited on the player, not on a timer: the button is gated on the bytes
    // being here, so checking before they are would pass for the wrong reason.
    await within(viewer).findByTestId('media-player-video');
    expect(within(viewer).queryByTestId('attachment-last-frame')).toBeNull();
  });

  it('offers no deletion on a surface that cannot refresh its list', async () => {
    render(
      <DiscussionAssetsPanel
        discussionId="disc-1"
        files={[file(1, { filename: 'dashboard.png', mime_type: 'image/png', disk_path: '/tmp/dashboard.png' })]}
        onClose={vi.fn()}
        onNavigateMessage={vi.fn()}
        openAssetRequest={{ assetId: 'file-1', nonce: 1 }}
        t={t}
      />,
    );
    const viewer = await screen.findByRole('dialog');
    expect(within(viewer).queryByTestId('attachment-delete')).toBeNull();
  });

});

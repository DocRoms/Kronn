import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, createEvent, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import type { ContextFile } from '../../types/generated';

const { discussionsApi } = vi.hoisted(() => ({
  discussionsApi: { videoSequence: vi.fn(), setVideoSequence: vi.fn(), contextFileBlob: vi.fn() },
}));

vi.mock('../../lib/api', () => ({ discussions: discussionsApi }));

import { VideoSequenceEditor } from '../VideoSequenceEditor';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}:${args.join(',')}` : key;

function clip(id: string, day: number): ContextFile {
  return {
    id,
    discussion_id: 'disc-1',
    filename: `${id}.mp4`,
    mime_type: 'video/mp4',
    original_size: 1,
    extracted_size: 1,
    disk_path: `/tmp/${id}.mp4`,
    message_id: `message-${id}`,
    ai_generation: null,
    created_at: `2026-09-0${day}T10:00:00Z`,
  };
}

/** A clip Kronn generated, with the length it measured and the price declared. */
function generated(id: string, day: number, durationMs: number | null, costUsd: number | null, isByok = false): ContextFile {
  return {
    ...clip(id, day),
    ai_generation: { model: 'provider/video', prompt: id, duration_ms: durationMs, cost_usd: costUsd, is_byok: isByok },
  };
}

const intro = clip('intro', 1);
const middle = clip('middle', 2);
const outro = clip('outro', 3);

function renderEditor(videos: ContextFile[] = [outro, intro, middle]) {
  return render(<VideoSequenceEditor discussionId="disc-1" videos={videos} t={t} />);
}

const names = (testId: string) => screen.queryAllByTestId(testId).map(item =>
  item.querySelector('.disc-sequence-name')?.textContent);
const listed = () => names('video-sequence-item');
const aside = () => names('video-sequence-aside-item');

const dataTransfer = { effectAllowed: '', dropEffect: '' };

/** The pointer over the upper or lower half of a 40 px row. */
function dragOverRow(row: HTMLElement, half: 'upper' | 'lower') {
  vi.spyOn(row, 'getBoundingClientRect').mockReturnValue(
    { top: 0, bottom: 40, height: 40, left: 0, right: 200, width: 200, x: 0, y: 0, toJSON: () => ({}) } as DOMRect,
  );
  const event = createEvent.dragOver(row, { dataTransfer });
  // jsdom drops the pointer position from the event's init.
  Object.defineProperty(event, 'clientY', { value: half === 'upper' ? 10 : 30 });
  fireEvent(row, event);
}

const rows = () => screen.getAllByTestId('video-sequence-item');
const lines = () => [...document.querySelectorAll('[data-drop]')]
  .map(row => `${row.getAttribute('data-drop')} ${row.querySelector('.disc-sequence-name')?.textContent}`);

/** Every row is on screen as soon as it is observed. */
class VisibleRows {
  constructor(private readonly callback: (entries: Array<{ isIntersecting: boolean }>) => void) {}
  observe() { this.callback([{ isIntersecting: true }]); }
  disconnect() {}
}

describe('VideoSequenceEditor', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal('IntersectionObserver', VisibleRows);
    discussionsApi.videoSequence.mockResolvedValue({ file_ids: [], excluded_ids: [] });
    discussionsApi.setVideoSequence.mockImplementation(
      (_id: string, fileIds: string[], excludedIds: string[]) =>
        Promise.resolve({ file_ids: fileIds, excluded_ids: excludedIds }));
    discussionsApi.contextFileBlob.mockImplementation((_id: string, fileId: string) =>
      Promise.resolve(new Blob([fileId], { type: 'video/mp4' })));
    let next = 0;
    globalThis.URL.createObjectURL = vi.fn(() => `blob:clip-${next++}`);
    globalThis.URL.revokeObjectURL = vi.fn();
  });

  afterEach(() => { vi.unstubAllGlobals(); });

  it('lists the clips in the order they were made until someone arranges them', async () => {
    renderEditor();
    expect(screen.getByRole('status')).toHaveTextContent('disc.assets.editorLoading');

    await screen.findByTestId('video-sequence-editor');
    expect(listed()).toEqual(['intro.mp4', 'middle.mp4', 'outro.mp4']);
    expect(discussionsApi.videoSequence).toHaveBeenCalledWith('disc-1');
  });

  it('shows the stored order, with a clip made since at the end', async () => {
    discussionsApi.videoSequence.mockResolvedValue({ file_ids: ['outro', 'intro'], excluded_ids: [] });
    renderEditor();

    await screen.findByTestId('video-sequence-editor');
    expect(listed()).toEqual(['outro.mp4', 'intro.mp4', 'middle.mp4']);
    expect(aside()).toEqual([]);
  });

  it('still lists the clips when the stored order cannot be read', async () => {
    discussionsApi.videoSequence.mockRejectedValue(new Error('offline'));
    renderEditor();

    await screen.findByTestId('video-sequence-editor');
    expect(listed()).toEqual(['intro.mp4', 'middle.mp4', 'outro.mp4']);
  });

  it('saves the new order as soon as a clip is moved', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    fireEvent.click(screen.getByRole('button', { name: 'disc.assets.moveDown:intro.mp4' }));
    expect(listed()).toEqual(['middle.mp4', 'intro.mp4', 'outro.mp4']);
    expect(discussionsApi.setVideoSequence).toHaveBeenCalledWith('disc-1', ['middle', 'intro', 'outro'], []);

    fireEvent.click(screen.getByRole('button', { name: 'disc.assets.moveUp:outro.mp4' }));
    expect(discussionsApi.setVideoSequence).toHaveBeenLastCalledWith('disc-1', ['middle', 'outro', 'intro'], []);
  });

  it('cannot move the first clip up nor the last one down', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    expect(screen.getByRole('button', { name: 'disc.assets.moveUp:intro.mp4' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'disc.assets.moveDown:outro.mp4' })).toBeDisabled();
  });

  it('shows where a dragged clip will land, and drops it there', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');
    const [first, , last] = rows();

    fireEvent.dragStart(last, { dataTransfer });
    expect(last).toHaveAttribute('data-dragging', 'true');
    dragOverRow(first, 'upper');
    expect(lines()).toEqual(['before intro.mp4']);
    // Reordering inside the film is the line's job, not the zone's.
    expect(screen.getByTestId('video-sequence-final')).toHaveAttribute('data-drop-target', 'false');

    fireEvent.drop(first, { dataTransfer });
    expect(listed()).toEqual(['outro.mp4', 'intro.mp4', 'middle.mp4']);
    expect(discussionsApi.setVideoSequence).toHaveBeenCalledWith('disc-1', ['outro', 'intro', 'middle'], []);
    expect(lines()).toEqual([]);
    expect(document.querySelector('[data-dragging]')).toBeNull();
  });

  it('draws the line under the last clip to send a clip to the end', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    fireEvent.dragStart(rows()[0], { dataTransfer });
    dragOverRow(rows()[2], 'lower');
    expect(lines()).toEqual(['after outro.mp4']);
    dragOverRow(rows()[2], 'upper');
    expect(lines()).toEqual(['before outro.mp4']);

    fireEvent.drop(rows()[2], { dataTransfer });
    expect(listed()).toEqual(['middle.mp4', 'intro.mp4', 'outro.mp4']);
  });

  it('draws no line where the clip would not move, and saves nothing there', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');
    const [first, middle, last] = rows();

    fireEvent.dragStart(middle, { dataTransfer });
    dragOverRow(first, 'lower');
    expect(lines()).toEqual([]);
    dragOverRow(last, 'upper');
    expect(lines()).toEqual([]);

    fireEvent.drop(last, { dataTransfer });
    expect(listed()).toEqual(['intro.mp4', 'middle.mp4', 'outro.mp4']);
    expect(discussionsApi.setVideoSequence).not.toHaveBeenCalled();
  });

  it('keeps the line while the pointer crosses the gap between two rows', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    fireEvent.dragStart(rows()[2], { dataTransfer });
    dragOverRow(rows()[0], 'lower');
    expect(lines()).toEqual(['before middle.mp4']);
    fireEvent.dragOver(screen.getByTestId('video-sequence-final').querySelector('ol') as HTMLElement, { dataTransfer });
    expect(lines()).toEqual(['before middle.mp4']);
  });

  it('forgets the line when the drag is abandoned', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    fireEvent.dragStart(rows()[2], { dataTransfer });
    dragOverRow(rows()[0], 'upper');
    fireEvent.dragEnd(rows()[2], { dataTransfer });

    expect(lines()).toEqual([]);
    expect(document.querySelector('[data-dragging]')).toBeNull();
    expect(discussionsApi.setVideoSequence).not.toHaveBeenCalled();
  });

  it('sets a clip aside, and brings it back at the end of the film', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');
    expect(screen.getByTestId('video-sequence-aside')).toHaveTextContent('disc.assets.excludedEmpty');

    fireEvent.click(screen.getByRole('button', { name: 'disc.assets.excludeClip:middle.mp4' }));
    expect(listed()).toEqual(['intro.mp4', 'outro.mp4']);
    expect(aside()).toEqual(['middle.mp4']);
    expect(discussionsApi.setVideoSequence).toHaveBeenLastCalledWith('disc-1', ['intro', 'outro'], ['middle']);
    expect(screen.getByText('disc.assets.editorHint:2')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'disc.assets.includeClip:middle.mp4' }));
    expect(listed()).toEqual(['intro.mp4', 'outro.mp4', 'middle.mp4']);
    expect(aside()).toEqual([]);
    expect(discussionsApi.setVideoSequence).toHaveBeenLastCalledWith('disc-1', ['intro', 'outro', 'middle'], []);
  });

  it('sets aside a clip dropped on the zone, and brings one back where it is dropped', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    fireEvent.dragStart(rows()[0], { dataTransfer });
    fireEvent.dragOver(screen.getByTestId('video-sequence-aside'), { dataTransfer });
    expect(screen.getByTestId('video-sequence-aside')).toHaveAttribute('data-drop-target', 'true');
    fireEvent.drop(screen.getByTestId('video-sequence-aside'), { dataTransfer });
    expect(listed()).toEqual(['middle.mp4', 'outro.mp4']);
    expect(aside()).toEqual(['intro.mp4']);
    expect(screen.getByTestId('video-sequence-aside')).toHaveAttribute('data-drop-target', 'false');

    fireEvent.dragStart(screen.getAllByTestId('video-sequence-aside-item')[0], { dataTransfer });
    dragOverRow(rows()[1], 'upper');
    expect(lines()).toEqual(['before outro.mp4']);
    expect(screen.getByTestId('video-sequence-final')).toHaveAttribute('data-drop-target', 'true');
    fireEvent.drop(rows()[1], { dataTransfer });
    expect(listed()).toEqual(['middle.mp4', 'intro.mp4', 'outro.mp4']);
    expect(discussionsApi.setVideoSequence).toHaveBeenLastCalledWith('disc-1', ['middle', 'intro', 'outro'], []);
  });

  it('keeps a clip set aside out of the film on reload', async () => {
    discussionsApi.videoSequence.mockResolvedValue({ file_ids: ['outro'], excluded_ids: ['intro'] });
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    // `middle` was made after the order was saved: it joins the film.
    expect(listed()).toEqual(['outro.mp4', 'middle.mp4']);
    expect(aside()).toEqual(['intro.mp4']);
  });

  it('has nothing to play once every clip is set aside', async () => {
    discussionsApi.videoSequence.mockResolvedValue({ file_ids: [], excluded_ids: ['intro', 'middle', 'outro'] });
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    expect(screen.getByTestId('video-sequence-final')).toHaveTextContent('disc.assets.finalEmpty');
    expect(screen.getByTestId('video-sequence-play')).toBeDisabled();
  });

  it('shows each clip by its first frame', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    const thumbs = screen.getAllByTestId('video-sequence-thumb');
    expect(thumbs).toHaveLength(3);
    await waitFor(() => expect(thumbs.every(thumb => thumb.querySelector('video'))).toBe(true));
    expect(thumbs[0].querySelector('video')).toHaveAttribute('src', expect.stringMatching(/^blob:/));
  });

  it('says the order was not kept when the server refuses it', async () => {
    discussionsApi.setVideoSequence.mockRejectedValue(new Error('disk full'));
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    fireEvent.click(screen.getByRole('button', { name: 'disc.assets.moveDown:intro.mp4' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('disc.assets.orderNotSaved:disk full');
  });

  it('plays the final cut clip after clip, in the arranged order', async () => {
    discussionsApi.videoSequence.mockResolvedValue({ file_ids: ['outro', 'intro', 'middle'], excluded_ids: [] });
    renderEditor();
    fireEvent.click(await screen.findByTestId('video-sequence-play'));

    const player = screen.getByTestId('video-sequence-player');
    // Out of the panel, whose overflow would clip a full-screen film.
    expect(player.parentElement).toBe(document.body);
    const position = within(player).getByTestId('video-sequence-position');
    expect(position).toHaveTextContent('disc.assets.clipPosition:1,3,outro.mp4');
    const firstVideo = await within(player).findByTestId('video-sequence-video');
    expect(firstVideo).toHaveAttribute('autoplay');
    await waitFor(() => expect(discussionsApi.contextFileBlob).toHaveBeenCalledWith('disc-1', 'intro'));

    fireEvent.ended(firstVideo);
    expect(position).toHaveTextContent('disc.assets.clipPosition:2,3,intro.mp4');
    fireEvent.ended(await within(player).findByTestId('video-sequence-video'));
    expect(position).toHaveTextContent('disc.assets.clipPosition:3,3,middle.mp4');
    fireEvent.ended(await within(player).findByTestId('video-sequence-video'));

    expect(position).toHaveTextContent('disc.assets.finalEnded:3');
    fireEvent.click(within(player).getByTestId('video-sequence-replay'));
    expect(position).toHaveTextContent('disc.assets.clipPosition:1,3,outro.mp4');
    // Once per clip, shared by the thumbnails and the player.
    expect(discussionsApi.contextFileBlob).toHaveBeenCalledTimes(3);
  });

  it('plays only the clips of the final cut', async () => {
    discussionsApi.videoSequence.mockResolvedValue({ file_ids: ['outro', 'intro'], excluded_ids: ['middle'] });
    renderEditor();
    fireEvent.click(await screen.findByTestId('video-sequence-play'));
    const player = screen.getByTestId('video-sequence-player');
    const position = within(player).getByTestId('video-sequence-position');

    expect(position).toHaveTextContent('disc.assets.clipPosition:1,2,outro.mp4');
    fireEvent.ended(await within(player).findByTestId('video-sequence-video'));
    expect(position).toHaveTextContent('disc.assets.clipPosition:2,2,intro.mp4');
    fireEvent.ended(await within(player).findByTestId('video-sequence-video'));
    expect(position).toHaveTextContent('disc.assets.finalEnded:2');
  });

  it('steps between clips by hand', async () => {
    renderEditor();
    fireEvent.click(await screen.findByTestId('video-sequence-play'));
    const player = screen.getByTestId('video-sequence-player');
    const position = within(player).getByTestId('video-sequence-position');

    expect(within(player).getByRole('button', { name: 'disc.assets.previousClip' })).toBeDisabled();
    fireEvent.click(within(player).getByRole('button', { name: 'disc.assets.nextClip' }));
    expect(position).toHaveTextContent('disc.assets.clipPosition:2,3,middle.mp4');
    fireEvent.click(within(player).getByRole('button', { name: 'disc.assets.previousClip' }));
    expect(position).toHaveTextContent('disc.assets.clipPosition:1,3,intro.mp4');
    await waitFor(() => expect(URL.createObjectURL).toHaveBeenCalledTimes(3));
  });

  it('offers to skip a clip that cannot be loaded', async () => {
    discussionsApi.contextFileBlob.mockImplementation((_id: string, fileId: string) =>
      fileId === 'intro'
        ? Promise.reject(new Error('gone'))
        : Promise.resolve(new Blob([fileId], { type: 'video/mp4' })));
    renderEditor();
    fireEvent.click(await screen.findByTestId('video-sequence-play'));
    const player = screen.getByTestId('video-sequence-player');

    const failure = await within(player).findByRole('alert');
    expect(failure).toHaveTextContent('disc.assets.clipUnavailable:intro.mp4');
    fireEvent.click(within(failure).getByRole('button', { name: 'disc.assets.nextClip' }));

    expect(within(player).getByTestId('video-sequence-position'))
      .toHaveTextContent('disc.assets.clipPosition:2,3,middle.mp4');
    expect(await within(player).findByTestId('video-sequence-video')).toBeInTheDocument();
    await waitFor(() => expect(URL.createObjectURL).toHaveBeenCalledTimes(2));
  });

  it('closes on Escape and on the close button', async () => {
    renderEditor();
    fireEvent.click(await screen.findByTestId('video-sequence-play'));
    await screen.findByTestId('video-sequence-video');

    act(() => { fireEvent.keyDown(window, { key: 'Escape' }); });
    expect(screen.queryByTestId('video-sequence-player')).toBeNull();

    fireEvent.click(screen.getByTestId('video-sequence-play'));
    fireEvent.click(await screen.findByTestId('video-sequence-close'));
    expect(screen.queryByTestId('video-sequence-player')).toBeNull();
    await waitFor(() => expect(URL.createObjectURL).toHaveBeenCalledTimes(3));
  });

  it('shows each clip\'s length and price, and what the final cut adds up to', async () => {
    renderEditor([generated('intro', 1, 6000, 0.07), generated('middle', 2, 8000, 0.12), generated('outro', 3, 5042, null)]);
    await screen.findByTestId('video-sequence-editor');

    expect(screen.getAllByTestId('video-sequence-meta').map(meta => meta.textContent)).toEqual([
      'disc.assets.durationSeconds:6 · $0.07',
      'disc.assets.durationSeconds:8 · $0.12',
      'disc.assets.durationSeconds:5',
    ]);
    expect(screen.getByTestId('video-sequence-total-duration')).toHaveTextContent('disc.assets.durationSeconds:19');
    const cost = screen.getByTestId('video-sequence-total-cost');
    expect(cost).toHaveTextContent('$0.19');
    // The unpriced clip is said, not counted as free.
    expect(cost).toHaveTextContent('disc.assets.uncountedCosts:1');
  });

  it('adds up only the clips of the final cut', async () => {
    discussionsApi.videoSequence.mockResolvedValue({ file_ids: [], excluded_ids: ['middle'] });
    renderEditor([generated('intro', 1, 6000, 0.07), generated('middle', 2, 8000, 0.12), generated('outro', 3, 5042, 0.05)]);
    await screen.findByTestId('video-sequence-editor');

    expect(screen.getByTestId('video-sequence-total-duration')).toHaveTextContent('disc.assets.durationSeconds:11');
    expect(screen.getByTestId('video-sequence-total-cost')).toHaveTextContent('$0.12');

    fireEvent.click(screen.getByRole('button', { name: 'disc.assets.includeClip:middle.mp4' }));
    expect(screen.getByTestId('video-sequence-total-duration')).toHaveTextContent('disc.assets.durationSeconds:19');
    expect(screen.getByTestId('video-sequence-total-cost')).toHaveTextContent('$0.24');
  });

  it('reads an upload\'s length from the clip itself', async () => {
    renderEditor([clip('intro', 1), generated('outro', 3, 58000, 0.4)]);
    await screen.findByTestId('video-sequence-editor');
    const total = screen.getByTestId('video-sequence-total-duration');
    expect(total).toHaveTextContent('disc.assets.durationSeconds:58');
    expect(total).toHaveTextContent('disc.assets.unknownDurations:1');

    const [uploadThumb] = screen.getAllByTestId('video-sequence-thumb');
    const video = await waitFor(() => {
      const found = uploadThumb.querySelector('video');
      if (!found) throw new Error('no thumbnail yet');
      return found;
    });
    Object.defineProperty(video, 'duration', { value: 7.2, configurable: true });
    fireEvent.loadedMetadata(video);

    // 58 s + 7 s: past a minute.
    expect(total).toHaveTextContent('disc.assets.durationMinutes:1,05');
    expect(total).not.toHaveTextContent('unknownDurations');
    expect(screen.getAllByTestId('video-sequence-meta')[0]).toHaveTextContent('disc.assets.durationSeconds:7');
  });

  it('says a clip was billed on the user\'s own key instead of pricing it', async () => {
    renderEditor([generated('intro', 1, 6000, 0.3, true), generated('outro', 3, 6000, 0.1)]);
    await screen.findByTestId('video-sequence-editor');

    expect(screen.getAllByTestId('video-sequence-meta')[0]).toHaveTextContent('disc.assets.durationSeconds:6 · run.media.byok');
    const cost = screen.getByTestId('video-sequence-total-cost');
    expect(cost).toHaveTextContent('$0.10');
    expect(cost).toHaveTextContent('disc.assets.uncountedCosts:1');
  });

  it('has no total to show when no clip of the film has a price', async () => {
    renderEditor();
    await screen.findByTestId('video-sequence-editor');

    expect(screen.getByTestId('video-sequence-total-cost')).toHaveTextContent('—');
    expect(screen.getByTestId('video-sequence-total-cost')).toHaveTextContent('disc.assets.uncountedCosts:3');
  });

  it('fetches a clip only once its row is on screen, or when it is played', async () => {
    vi.stubGlobal('IntersectionObserver', class {
      observe() {}
      disconnect() {}
    });
    renderEditor();
    await screen.findByTestId('video-sequence-editor');
    expect(discussionsApi.contextFileBlob).not.toHaveBeenCalled();

    fireEvent.click(screen.getByTestId('video-sequence-play'));
    await screen.findByTestId('video-sequence-video');
    await waitFor(() => expect(discussionsApi.contextFileBlob).toHaveBeenCalledTimes(2));
    expect(discussionsApi.contextFileBlob).toHaveBeenCalledWith('disc-1', 'intro');
    expect(discussionsApi.contextFileBlob).toHaveBeenCalledWith('disc-1', 'middle');
  });

  it('releases the clips it loaded when the editor closes', async () => {
    const { unmount } = renderEditor();
    await screen.findByTestId('video-sequence-editor');
    await waitFor(() => expect(URL.createObjectURL).toHaveBeenCalledTimes(3));

    unmount();
    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(3);
  });
});

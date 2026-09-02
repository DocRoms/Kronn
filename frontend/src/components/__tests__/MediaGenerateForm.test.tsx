import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ExternalApiConnectionView } from '../../lib/api';

const { mediaApi } = vi.hoisted(() => ({
  mediaApi: { generate: vi.fn(), estimate: vi.fn(), capabilities: vi.fn() },
}));

const contextFileBlob = vi.fn();
const uploadContextFile = vi.fn();
vi.mock('../../lib/api', () => ({
  media: mediaApi,
  discussions: {
    contextFileBlob: (...a: unknown[]) => contextFileBlob(...a),
    uploadContextFile: (...a: unknown[]) => uploadContextFile(...a),
  },
}));

import { MediaGenerateForm } from '../MediaGenerateForm';

const t = (key: string, ...args: (string | number)[]) =>
  args.length ? `${key}:${args.join(',')}` : key;

function connection(over: Partial<ExternalApiConnectionView> = {}): ExternalApiConnectionView {
  return {
    id: 'conn-1',
    display_name: 'OpenRouter',
    mention_alias: '@openrouter',
    endpoint: 'https://openrouter.ai/api/v1',
    origin_preset: 'open_router',
    has_credential: true,
    economy_model: null,
    default_model: null,
    reasoning_model: null,
    image_model: 'google/gemini-2.5-flash-image',
    video_model: 'bytedance/seedance-2.0-mini',
    media_endpoint: null,
    created_at: '2026-08-31T10:00:00Z',
    updated_at: '2026-08-31T10:00:00Z',
    ...over,
  } as ExternalApiConnectionView;
}

describe('MediaGenerateForm', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mediaApi.estimate.mockResolvedValue({ model: 'x', estimated_usd: 0.0709, samples: 3 });
    // Default: a provider that advertises nothing, so the form keeps its
    // fallback lists. Tests that care about the catalogue override this.
    mediaApi.capabilities.mockResolvedValue({ model: 'x', capabilities: null });
    contextFileBlob.mockResolvedValue(new Blob(['x']));
    // jsdom decodes nothing; the width is what the test says the picture has.
    globalThis.createImageBitmap = vi.fn(async () => ({ width: 512, height: 512, close: vi.fn() })) as never;
    URL.createObjectURL = vi.fn(() => 'blob:thumb');
    URL.revokeObjectURL = vi.fn();
    mediaApi.generate.mockResolvedValue({
      job_id: 'job-1',
      status: 'pending',
      model: 'bytedance/seedance-2.0-mini',
      discussion_id: 'd-1',
      message_id: 'msg-anchor-1',
    });
  });

  it('sends the prompt and the shape, never a model', async () => {
    const onLaunched = vi.fn();
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        t={t}
        onLaunched={onLaunched}
      />,
    );

    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    fireEvent.change(screen.getByRole('textbox'), {
      target: { value: 'un chat en origami' },
    });
    fireEvent.click(screen.getByRole('button', { name: /disc\.media\.generate/ }));

    await waitFor(() => expect(mediaApi.generate).toHaveBeenCalledTimes(1));
    const body = mediaApi.generate.mock.calls[0][0];
    expect(body).toMatchObject({
      connection_id: 'conn-1',
      modality: 'video',
      prompt: 'un chat en origami',
      discussion_id: 'd-1',
      duration_secs: 5,
      // Sent explicitly even at its default value: an absent field leaves the
      // provider free to add a soundtrack nobody asked for.
      generate_audio: true,
    });
    // The slot decides the model: a caller-supplied one would let the UI bill
    // something the operator never configured.
    expect(body).not.toHaveProperty('model');
    expect(onLaunched).toHaveBeenCalledWith('job-1', 'msg-anchor-1');
    expect(
      await screen.findByText('disc.media.launched:bytedance/seedance-2.0-mini'),
    ).toBeInTheDocument();
  });

  it('shows the price of the click, and says so when there is none', async () => {
    const { unmount } = render(
      <MediaGenerateForm discussionId="d-1" connections={[connection()]} t={t} />,
    );
    expect(await screen.findByText('disc.media.estimate:0.0709,3')).toBeInTheDocument();
    unmount();

    // A model billed nothing yet must read as unknown, not as free.
    mediaApi.estimate.mockResolvedValue({ model: 'x', estimated_usd: null, samples: 0 });
    render(<MediaGenerateForm discussionId="d-1" connections={[connection()]} t={t} />);
    expect(await screen.findByText('disc.media.estimateUnknown')).toBeInTheDocument();
  });

  it('offers one entry per configured model, with no modality question', async () => {
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[
          connection({ id: 'img-only', display_name: 'Images only', video_model: null }),
          connection({ id: 'vid-only', display_name: 'Videos only', image_model: null }),
        ]}
        t={t}
      />,
    );

    // One card per configured slot; the modality is a property of the choice,
    // never a separate question.
    expect(screen.getByTestId('media-slot-img-only:image')).toBeInTheDocument();
    expect(screen.getByTestId('media-slot-vid-only:video')).toBeInTheDocument();
    expect(screen.queryByTestId('media-slot-img-only:video')).toBeNull();
    expect(screen.queryByTestId('media-slot-vid-only:image')).toBeNull();
    // The first slot is preselected, so a prompt is enough to launch.
    expect(screen.getByTestId('media-slot-img-only:image')).toHaveAttribute(
      'aria-checked',
      'true',
    );

    // Duration only exists for a clip, and the image slot is the active one.
    expect(screen.queryByText('disc.media.duration')).toBeNull();
    fireEvent.click(screen.getByTestId('media-slot-vid-only:video'));
    await waitFor(() => expect(screen.getByText('disc.media.duration')).toBeInTheDocument());
  });

  it('shows each aspect ratio as a shape, not just as arithmetic', async () => {
    render(
      <MediaGenerateForm discussionId="d-1" connections={[connection()]} t={t} />,
    );
    // A proportional box per ratio: `4:3` is unreadable to most people until
    // they see it.
    for (const ratio of ['16:9', '4:3', '1:1', '9:16']) {
      const choice = screen.getByTestId(`media-ratio-${ratio}`);
      expect(choice.querySelector('.media-generate-ratio-shape')).toHaveAttribute(
        'data-ratio',
        ratio,
      );
    }
    expect(screen.getByTestId('media-ratio-16:9')).toHaveAttribute('aria-checked', 'true');
    // Let the asynchronous estimate settle inside the test so React does not
    // report a state update after the assertion phase has already ended.
    expect(await screen.findByText('disc.media.estimate:0.0709,3')).toBeInTheDocument();
  });

  it('offers the soundtrack as a visible, checked-by-default choice', async () => {
    render(
      <MediaGenerateForm discussionId="d-1" connections={[connection()]} t={t} />,
    );

    // Sound is a video question only; an image slot must not ask it.
    expect(screen.queryByTestId('media-generate-audio')).toBeNull();
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    const audio = await screen.findByTestId('media-generate-audio');
    // Checked by default: the box makes the current provider behaviour
    // visible, it does not change it.
    expect(audio).toBeChecked();

    fireEvent.click(audio);
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'un plan muet' } });
    fireEvent.click(screen.getByRole('button', { name: /disc\.media\.generate/ }));

    await waitFor(() => expect(mediaApi.generate).toHaveBeenCalledTimes(1));
    expect(mediaApi.generate.mock.calls[0][0]).toMatchObject({ generate_audio: false });
  });

  it('never asks for a soundtrack on an image', async () => {
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection({ video_model: null })]}
        t={t}
      />,
    );
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'un chat en origami' } });
    fireEvent.click(screen.getByRole('button', { name: /disc\.media\.generate/ }));

    await waitFor(() => expect(mediaApi.generate).toHaveBeenCalledTimes(1));
    // A picture has no soundtrack: sending the field would be noise the
    // backend has to ignore.
    expect(mediaApi.generate.mock.calls[0][0]).not.toHaveProperty('generate_audio');
  });

  it('offers what the provider accepts, not a hard-coded list', async () => {
    // Measured on `bytedance/seedance-2.0-mini`: it refuses 3 s and 1080p,
    // and accepts seven ratios. The form used to offer the first two and hide
    // three of the seven — a billable click that could only fail.
    mediaApi.capabilities.mockResolvedValue({
      model: 'bytedance/seedance-2.0-mini',
      capabilities: {
        model: 'bytedance/seedance-2.0-mini',
        modality: 'video',
        durations_secs: [4, 5, 6, 7, 8],
        resolutions: ['480p', '720p'],
        aspect_ratios: ['1:1', '3:4', '9:16', '4:3', '16:9', '21:9', '9:21'],
        frame_positions: ['first_frame', 'last_frame'],
        max_input_references: null,
        generate_audio: true,
      },
    });
    render(<MediaGenerateForm discussionId="d-1" connections={[connection()]} t={t} />);
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));

    await waitFor(() => expect(screen.getByTestId('media-ratio-21:9')).toBeInTheDocument());
    const durations = screen.getByLabelText('disc.media.duration') as HTMLSelectElement;
    expect([...durations.options].map(option => option.value)).toEqual(['4', '5', '6', '7', '8']);
    const resolutions = screen.getByLabelText('disc.media.resolution') as HTMLSelectElement;
    expect([...resolutions.options].map(option => option.value)).toEqual(['480p', '720p']);
    expect(screen.queryByTestId('media-ratio-2:3')).toBeNull();
  });

  it('never submits a choice the newly selected model rejects', async () => {
    // 5 s and 480p are the form's own defaults. A model that accepts neither
    // must not receive them just because nobody touched the controls.
    mediaApi.capabilities.mockResolvedValue({
      model: 'strict/model',
      capabilities: {
        model: 'strict/model',
        modality: 'video',
        durations_secs: [10, 12],
        resolutions: ['1080p'],
        aspect_ratios: ['9:16'],
        frame_positions: [],
        max_input_references: null,
        generate_audio: false,
      },
    });
    render(<MediaGenerateForm discussionId="d-1" connections={[connection()]} t={t} />);
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-ratio-9:16')).toBeInTheDocument());

    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'un plan strict' } });
    fireEvent.click(screen.getByRole('button', { name: /disc\.media\.generate/ }));
    await waitFor(() => expect(mediaApi.generate).toHaveBeenCalledTimes(1));
    expect(mediaApi.generate.mock.calls[0][0]).toMatchObject({
      duration_secs: 10,
      resolution: '1080p',
      aspect_ratio: '9:16',
    });
    // This model names no soundtrack switch, so the box is not shown — and
    // nothing is asserted about audio it never offered.
    expect(screen.queryByTestId('media-generate-audio')).toBeNull();
  });

  const videoCapabilities = (frames: string[]) => ({
    model: 'bytedance/seedance-2.0-mini',
    capabilities: {
      model: 'bytedance/seedance-2.0-mini',
      modality: 'video',
      durations_secs: [4, 5],
      resolutions: ['480p'],
      aspect_ratios: ['16:9'],
      frame_positions: frames,
      max_input_references: null,
      generate_audio: true,
    },
  });

  /** What an IMAGE model advertises: a ceiling on reference images, and no
   *  frames at all — a picture has no first or last one. */
  const imageCapabilities = (maxReferences: number | null) => ({
    model: 'google/gemini-3-pro-image',
    capabilities: {
      model: 'google/gemini-3-pro-image',
      modality: 'image',
      durations_secs: [],
      resolutions: ['1K'],
      aspect_ratios: ['1:1'],
      frame_positions: [],
      max_input_references: maxReferences,
      generate_audio: null,
    },
  });

  const image = (id: string, filename: string) => ({
    id,
    discussion_id: 'd-1',
    filename,
    mime_type: 'image/png',
    original_size: 2048,
    extracted_size: 0,
    disk_path: `/tmp/${id}.png`,
    message_id: null,
    ai_generation: null,
    created_at: '2026-09-01T10:00:00Z',
  });

  it('starts a clip from an image of this room, by id and never by path', async () => {
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame', 'last_frame']));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());

    fireEvent.click(screen.getByTestId('media-reference-pick-asset-1'));
    fireEvent.click(await screen.findByTestId('media-reference-mode-last_frame'));
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'un renard' } });
    fireEvent.click(screen.getByRole('button', { name: /disc\.media\.generate/ }));

    await waitFor(() => expect(mediaApi.generate).toHaveBeenCalledTimes(1));
    const body = mediaApi.generate.mock.calls[0][0];
    expect(body).toMatchObject({ reference_asset_ids: ['asset-1'], reference_mode: 'last_frame' });
    // The browser sends an id. A path would tell it where the file lives and
    // let it point a generation outside this room.
    expect(JSON.stringify(body)).not.toContain('/tmp/');
  });

  it('refuses a starting picture the provider is too narrow to accept', async () => {
    // Measured on 02/09: an 8x8 source came back `400 InvalidParameter —
    // expected the width to be at least 300px`. The refusal is unbilled, but
    // it costs a launch that could only fail, so it is caught here instead.
    globalThis.createImageBitmap = vi.fn(async () => ({ width: 128, height: 128, close: vi.fn() })) as never;
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame', 'last_frame']));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'tiny.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('media-reference-pick-asset-1'));

    expect(await screen.findByTestId('media-reference-too-narrow'))
      .toHaveTextContent('disc.media.referenceTooNarrow:128,300');
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'un renard' } });
    const generate = screen.getByRole('button', { name: /disc\.media\.generate/ });
    expect(generate).toBeDisabled();
    fireEvent.click(generate);
    expect(mediaApi.generate).not.toHaveBeenCalled();
  });

  it('launches on a picture that cannot be measured rather than blocking it', async () => {
    // An unmeasurable picture is not a small one: refusing it would hide a
    // source the provider would have accepted.
    globalThis.createImageBitmap = vi.fn(async () => { throw new Error('no decoder'); }) as never;
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame']));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('media-reference-pick-asset-1'));
    await screen.findByTestId('media-reference-mode-first_frame');
    expect(screen.queryByTestId('media-reference-too-narrow')).toBeNull();

    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'un renard' } });
    fireEvent.click(screen.getByRole('button', { name: /disc\.media\.generate/ }));
    await waitFor(() => expect(mediaApi.generate).toHaveBeenCalledTimes(1));
  });

  it('sends several reference images for an image, in the order they were picked', async () => {
    // Measured on the public catalogue: `google/gemini-3-pro-image` advertises
    // up to 14. Providers weigh references by position, so the order the
    // operator chose is the order that must leave.
    mediaApi.capabilities.mockResolvedValue(imageCapabilities(3));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png'), image('asset-2', 'renard.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:image'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());

    fireEvent.click(screen.getByTestId('media-reference-pick-asset-2'));
    fireEvent.click(await screen.findByTestId('media-reference-pick-asset-1'));
    // A frame is a video notion: nothing about one may appear here.
    expect(screen.queryByTestId('media-reference-mode-first_frame')).toBeNull();

    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'un renard origami' } });
    fireEvent.click(screen.getByRole('button', { name: /disc\.media\.generate/ }));

    await waitFor(() => expect(mediaApi.generate).toHaveBeenCalledTimes(1));
    expect(mediaApi.generate.mock.calls[0][0]).toMatchObject({
      reference_asset_ids: ['asset-2', 'asset-1'],
      reference_mode: 'reference',
    });
  });

  it('stops at the ceiling the model advertises, and says so', async () => {
    // `microsoft/mai-image-2.5-pro` and the krea models advertise exactly one.
    // A picker that simply stopped responding would read as broken.
    mediaApi.capabilities.mockResolvedValue(imageCapabilities(1));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png'), image('asset-2', 'renard.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:image'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('media-reference-pick-asset-1'));

    expect(await screen.findByTestId('media-reference-limit'))
      .toHaveTextContent('disc.media.referenceLimitReached:1');
    expect(screen.queryByTestId('media-reference-pick-asset-2')).toBeNull();
  });

  it('offers no reference at all when the image model advertises none', async () => {
    mediaApi.capabilities.mockResolvedValue(imageCapabilities(null));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:image'));
    await waitFor(() => expect(screen.getByTestId('media-generate-form')).toBeInTheDocument());
    expect(screen.queryByTestId('media-reference-picker')).toBeNull();
  });

  it('trims the pictures a newly selected model cannot take', async () => {
    // Trimmed, not dropped: what the narrower model still accepts survives the
    // switch, and only the excess goes. Two image connections, two ceilings.
    mediaApi.capabilities.mockImplementation(async (connectionId: string) =>
      connectionId === 'conn-1' ? imageCapabilities(3) : imageCapabilities(1));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[
          connection({ video_model: null }),
          connection({ id: 'conn-2', display_name: 'Krea', video_model: null, image_model: 'krea/krea-2-large' }),
        ]}
        images={[image('asset-1', 'a.png'), image('asset-2', 'b.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:image'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('media-reference-pick-asset-1'));
    fireEvent.click(await screen.findByTestId('media-reference-pick-asset-2'));
    expect(await screen.findByTestId('media-reference-drop-asset-2')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('media-slot-conn-2:image'));

    await waitFor(() => expect(screen.queryByTestId('media-reference-drop-asset-2')).toBeNull());
    // The first picture stays: the new model still takes one.
    expect(screen.getByTestId('media-reference-drop-asset-1')).toBeInTheDocument();
  });

  it('narrows a long list of pictures instead of asking to scroll it', async () => {
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame']));
    const many = ['renard.png', 'origami.png', 'chat.png', 'ville.png', 'foret.png', 'mer.png']
      .map((filename, index) => image(`asset-${index}`, filename));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={many as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());

    fireEvent.change(screen.getByTestId('media-reference-search'), { target: { value: 'ori' } });
    expect(screen.getByTestId('media-reference-pick-asset-1')).toBeInTheDocument();
    expect(screen.queryByTestId('media-reference-pick-asset-0')).toBeNull();

    // A search that matches nothing says so: an empty row reads as a broken
    // picker.
    fireEvent.change(screen.getByTestId('media-reference-search'), { target: { value: 'zzz' } });
    expect(screen.getByTestId('media-reference-no-match')).toBeInTheDocument();
  });

  it('attaches a new picture and picks it without a second step', async () => {
    const attached = { ...image('asset-new', 'nouvelle.png'), mime_type: 'image/png' };
    uploadContextFile.mockResolvedValue({ file: attached });
    const onImageAttached = vi.fn();
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame']));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png')] as never}
        t={t}
        onImageAttached={onImageAttached}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());

    const input = screen.getByTestId('media-reference-attach').querySelector('input')!;
    const file = new File(['png'], 'nouvelle.png', { type: 'image/png' });
    fireEvent.change(input, { target: { files: [file] } });

    await waitFor(() => expect(uploadContextFile).toHaveBeenCalledWith('d-1', file));
    expect(onImageAttached).toHaveBeenCalledWith(attached);
    // Attaching one here IS asking to use it; finding it again in the list
    // would be the extra step this control removes.
    expect(await screen.findByTestId('media-reference-mode-first_frame')).toBeInTheDocument();
  });

  it('still offers to attach the first picture in a room that holds none', async () => {
    // Hiding the picker on an empty room made attaching the FIRST picture
    // impossible — which is exactly the state a fresh discussion is in.
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame']));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[] as never}
        t={t}
        onImageAttached={vi.fn()}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    expect(await screen.findByTestId('media-reference-attach')).toBeInTheDocument();
  });

  it('offers no attachment on a surface that cannot show the new file', async () => {
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame']));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());
    expect(screen.queryByTestId('media-reference-attach')).toBeNull();
  });

  it('offers no source image when the model takes none', async () => {
    // Four of the 28 video models advertise no frame at all; offering the
    // picker there would promise a mode the provider refuses.
    mediaApi.capabilities.mockResolvedValue(videoCapabilities([]));
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-ratio-16:9')).toBeInTheDocument());
    expect(screen.queryByTestId('media-reference-picker')).toBeNull();
  });

  it('drops a picked image the newly selected model cannot take', async () => {
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame', 'last_frame']));
    const { rerender } = render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection()]}
        images={[image('asset-1', 'origami.png')] as never}
        t={t}
      />,
    );
    fireEvent.click(screen.getByTestId('media-slot-conn-1:video'));
    await waitFor(() => expect(screen.getByTestId('media-reference-picker')).toBeInTheDocument());
    fireEvent.click(screen.getByTestId('media-reference-pick-asset-1'));
    fireEvent.click(await screen.findByTestId('media-reference-mode-last_frame'));

    // The operator switches to a model that only takes a first frame: the
    // stored choice would be submitted as-is and refused after billing.
    mediaApi.capabilities.mockResolvedValue(videoCapabilities(['first_frame']));
    rerender(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection({ video_model: 'alibaba/wan-3.0' })]}
        images={[image('asset-1', 'origami.png')] as never}
        t={t}
      />,
    );
    // The picker briefly disappears while the new envelope is being read, so
    // the choice has to be awaited rather than read on the next tick.
    expect(await screen.findByTestId('media-reference-pick-asset-1')).toBeInTheDocument();
    expect(screen.queryByTestId('media-reference-mode-last_frame')).toBeNull();
  });

  it('explains itself when no connection has a media model', () => {
    render(
      <MediaGenerateForm
        discussionId="d-1"
        connections={[connection({ image_model: null, video_model: null })]}
        t={t}
      />,
    );
    expect(screen.getByText('disc.media.noSlot')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /disc\.media\.generate/ })).toBeNull();
  });
});

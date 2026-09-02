import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ExternalApiConnectionView } from '../../lib/api';

const { mediaApi } = vi.hoisted(() => ({
  mediaApi: { generate: vi.fn(), estimate: vi.fn(), capabilities: vi.fn() },
}));

vi.mock('../../lib/api', () => ({ media: mediaApi }));

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

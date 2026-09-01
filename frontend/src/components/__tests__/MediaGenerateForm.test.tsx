import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ExternalApiConnectionView } from '../../lib/api';

const { mediaApi } = vi.hoisted(() => ({
  mediaApi: { generate: vi.fn(), estimate: vi.fn() },
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
    mediaApi.generate.mockResolvedValue({
      job_id: 'job-1',
      status: 'pending',
      model: 'bytedance/seedance-2.0-mini',
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
    });
    // The slot decides the model: a caller-supplied one would let the UI bill
    // something the operator never configured.
    expect(body).not.toHaveProperty('model');
    expect(onLaunched).toHaveBeenCalledWith('job-1');
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

  it('shows each aspect ratio as a shape, not just as arithmetic', () => {
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

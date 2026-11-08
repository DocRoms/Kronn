import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';
import { MediaReferenceSettings } from '../MediaReferenceSettings';

const api = vi.hoisted(() => ({ capabilities: vi.fn(), setReferenceLimit: vi.fn() }));
vi.mock('../../../lib/api', () => buildApiMock({ media: api as never }));
const t = (key: string, ...args: (string | number)[]) => `${key}${args.length ? `:${args.join(',')}` : ''}`;
const envelope = (max: number | null, configured: number | null = null) => ({
  model: 'model-a', capabilities: {
    model: 'model-a', modality: 'image', durations_secs: [], resolutions: [], aspect_ratios: [],
    frame_positions: [], max_input_references: max, generate_audio: null,
    reference_support: { source: configured === null ? 'unknown' : 'configured', advertised_max: null, configured_max: configured, blocker: null },
  },
});
const open = () => {
  const details = screen.getByText('media.references.title:comfyui.slot.image').closest('details')!;
  details.open = true;
  fireEvent(details, new Event('toggle'));
};

beforeEach(() => {
  vi.resetAllMocks();
  api.capabilities.mockResolvedValue(envelope(null));
  api.setReferenceLimit.mockResolvedValue(envelope(3, 3));
});
afterEach(cleanup);

describe('model reference settings', () => {
  it('distinguishes unknown from zero, saves a limit and restores automatic detection', async () => {
    render(<MediaReferenceSettings connectionId="conn" model="model-a" modality="image" t={t} />);
    expect(api.capabilities).not.toHaveBeenCalled();
    open();
    await screen.findByText('media.references.unknown');
    fireEvent.change(screen.getByLabelText('media.references.limit'), { target: { value: '3' } });
    fireEvent.click(screen.getByText('media.references.save'));
    await screen.findByText('media.references.effective:3');
    expect(api.setReferenceLimit).toHaveBeenCalledWith({ connection_id: 'conn', model: 'model-a', modality: 'image', max_input_references: 3 });
    api.setReferenceLimit.mockResolvedValue(envelope(0, 0));
    fireEvent.change(screen.getByLabelText('media.references.limit'), { target: { value: '0' } });
    fireEvent.click(screen.getByText('media.references.save'));
    await screen.findByText('media.references.effective:0');
    api.setReferenceLimit.mockResolvedValue(envelope(null));
    fireEvent.change(screen.getByLabelText('media.references.limit'), { target: { value: '' } });
    fireEvent.click(screen.getByText('media.references.save'));
    await screen.findByText('media.references.unknown');
    expect(api.setReferenceLimit).toHaveBeenLastCalledWith(expect.objectContaining({ max_input_references: null }));
  });

  it('guards repeated submissions and surfaces a refused limit', async () => {
    let reject!: (error: Error) => void;
    api.setReferenceLimit.mockReturnValue(new Promise((_, fail) => { reject = fail; }));
    render(<MediaReferenceSettings connectionId="conn" model="model-a" modality="image" t={t} />);
    open();
    await screen.findByText('media.references.unknown');
    fireEvent.change(screen.getByLabelText('media.references.limit'), { target: { value: '65' } });
    expect(screen.getByText('media.references.save')).toBeDisabled();
    fireEvent.change(screen.getByLabelText('media.references.limit'), { target: { value: '3' } });
    act(() => { fireEvent.click(screen.getByText('media.references.save')); fireEvent.click(screen.getByText('media.references.save')); });
    expect(api.setReferenceLimit).toHaveBeenCalledTimes(1);
    await act(async () => { reject(new Error('The workflow accepts at most 1 image')); });
    expect(screen.getByRole('alert')).toHaveTextContent('at most 1');
    expect(screen.getByText('media.references.save')).toBeEnabled();
  });

  it('does not apply capabilities for a changed model', async () => {
    api.capabilities.mockResolvedValue({ ...envelope(3), model: 'model-b' });
    render(<MediaReferenceSettings connectionId="conn" model="model-a" modality="image" t={t} />);
    open();
    await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('media.references.modelChanged'));
    expect(screen.getByText('media.references.save')).toBeDisabled();
  });

  it('shows the missing ComfyUI dependency and its official installation guide', async () => {
    const result = envelope(0);
    api.capabilities.mockResolvedValue({ ...result, capabilities: { ...result.capabilities,
      reference_support: { source: 'workflow', advertised_max: 3, configured_max: null, blocker: 'Missing style_models/flux1-redux-dev.safetensors' },
    } });
    render(<MediaReferenceSettings connectionId="local:comfyui" model="model-a" modality="image" t={t} />);
    open();
    await screen.findByText('Missing style_models/flux1-redux-dev.safetensors');
    expect(screen.getByRole('link')).toHaveAttribute('href', 'https://comfyanonymous.github.io/ComfyUI_examples/flux/#redux');
  });
});

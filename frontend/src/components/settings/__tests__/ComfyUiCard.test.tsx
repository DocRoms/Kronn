import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';
import type { ComfyUiStatus } from '../../../types/generated';

const { status, setSlots } = vi.hoisted(() => ({ status: vi.fn(), setSlots: vi.fn() }));
vi.mock('../../../lib/api', () => buildApiMock({
  comfyui: { status: status as never, setSlots: setSlots as never },
}));
import { ComfyUiCard } from '../ComfyUiCard';

const t = (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key;

function online(patch: Partial<ComfyUiStatus> = {}): ComfyUiStatus {
  return {
    state: 'online', endpoint: 'http://127.0.0.1:8188', version: '0.42.1',
    models: [
      { folder: 'checkpoints', file: 'RealVisXL_V5.0_fp16.safetensors', family: 'sdxl', modality: 'image' },
      { folder: 'checkpoints', file: 'flux1-schnell-fp8.safetensors', family: 'flux_schnell', modality: 'image' },
      { folder: 'checkpoints', file: 'mystery.safetensors', family: null, modality: null },
      { folder: 'diffusion_models', file: 'wan2.1_t2v_1.3B_fp16.safetensors', family: 'wan21_t2v', modality: 'video' },
    ],
    image_model: null, video_model: null, video_blocker: null,
    install_command: 'brew install --cask comfy', download_url: 'https://www.comfy.org/download',
    ...patch,
  };
}

beforeEach(() => {
  status.mockReset();
  setSlots.mockReset();
});
afterEach(cleanup);

describe('ComfyUiCard (KT-1381)', () => {
  it('offers the install command and the first-launch step when ComfyUI is not found', async () => {
    status.mockResolvedValue({ ...online({ state: 'not_found', version: null, models: [] }) });
    await act(async () => { render(<ComfyUiCard t={t} />); });
    expect(await screen.findByText('brew install --cask comfy')).toBeInTheDocument();
    expect(screen.getByText('comfyui.firstLaunch')).toBeInTheDocument();
    expect(screen.getByText('comfyui.notFound')).toBeInTheDocument();
  });

  it('asks to start ComfyUI when it is installed but down', async () => {
    status.mockResolvedValue(online({ state: 'offline', version: null, models: [] }));
    await act(async () => { render(<ComfyUiCard t={t} />); });
    expect(await screen.findByText('comfyui.launchDesc:http://127.0.0.1:8188')).toBeInTheDocument();
  });

  it('lists only image models in the Image picker, an unknown one disabled, and saves the choice', async () => {
    status.mockResolvedValue(online());
    setSlots.mockResolvedValue({ image_model: 'RealVisXL_V5.0_fp16.safetensors', video_model: null });
    await act(async () => { render(<ComfyUiCard t={t} />); });
    expect(await screen.findByText('comfyui.online:0.42.1')).toBeInTheDocument();
    const image = screen.getByTestId('comfyui-slot-image');
    fireEvent.focus(image);
    expect(screen.getByRole('option', { name: /flux1-schnell/ })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: /mystery.*comfyui.unrecognised/ })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByRole('option', { name: /wan2\.1/ })).not.toBeInTheDocument();
    await act(async () => { fireEvent.click(screen.getByRole('option', { name: /RealVisXL/ })); });
    await waitFor(() => expect(setSlots).toHaveBeenCalledWith({
      image_model: 'RealVisXL_V5.0_fp16.safetensors', video_model: null,
    }));
  });

  it('lists only video models in the Video picker and says what a chosen one still lacks', async () => {
    status.mockResolvedValue(online({
      video_model: 'wan2.1_t2v_1.3B_fp16.safetensors',
      video_blocker: 'ComfyUI has no Wan VAE: add wan_2.1_vae.safetensors to models/vae',
    }));
    await act(async () => { render(<ComfyUiCard t={t} />); });
    expect(await screen.findByRole('alert')).toHaveTextContent('wan_2.1_vae');
    const video = screen.getByTestId('comfyui-slot-video');
    fireEvent.focus(video);
    expect(screen.queryByRole('option', { name: /RealVisXL/ })).not.toBeInTheDocument();
  });
});

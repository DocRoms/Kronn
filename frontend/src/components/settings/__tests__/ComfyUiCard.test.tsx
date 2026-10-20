import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';
import type { ComfyUiStatus } from '../../../types/generated';

const { status, setSlots, pull } = vi.hoisted(() => ({ status: vi.fn(), setSlots: vi.fn(), pull: vi.fn() }));
vi.mock('../../../lib/api', () => buildApiMock({
  comfyui: { status: status as never, setSlots: setSlots as never, pull: pull as never },
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
    suggestions: [],
    ...patch,
  };
}

beforeEach(() => {
  status.mockReset();
  setSlots.mockReset();
  pull.mockReset();
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

  // KT-1385 — suggested models download from the card, like Ollama's.
  it('downloads a suggestion with progress and reads the state again when done', async () => {
    const suggestions = [
      { id: 'realvisxl-v5', family: 'sdxl', modality: 'image', bytes: 6_938_065_488, installed: true, downloadable: true },
      { id: 'wan21-t2v-1.3b', family: 'wan21_t2v', modality: 'video', bytes: 9_828_025_775, installed: false, downloadable: true },
    ] as ComfyUiStatus['suggestions'];
    status.mockResolvedValue(online({ suggestions }));
    let handlers: { onProgress: (p: unknown) => void; onSuccess: () => void } | null = null;
    pull.mockImplementation((_id: string, h: typeof handlers) => { handlers = h; return Promise.resolve(); });
    await act(async () => { render(<ComfyUiCard t={t} />); });

    expect(screen.getByTestId('comfyui-suggestion-realvisxl-v5')).toHaveTextContent('comfyui.installed');
    expect(screen.queryByTestId('comfyui-pull-realvisxl-v5')).toBeNull();
    await act(async () => { fireEvent.click(screen.getByTestId('comfyui-pull-wan21-t2v-1.3b')); });
    expect(pull).toHaveBeenCalledWith('wan21-t2v-1.3b', expect.anything(), expect.any(AbortSignal));
    await act(async () => { handlers!.onProgress({ status: 'downloading', completed: 4_000_000_000, total: 9_828_025_775 }); });
    expect(screen.getByRole('status')).toHaveTextContent('4 GB');
    await act(async () => { handlers!.onSuccess(); });
    await waitFor(() => expect(status).toHaveBeenCalledTimes(2));
  });

  it('points to Hugging Face when Kronn cannot write into ComfyUI from here', async () => {
    status.mockResolvedValue(online({
      suggestions: [{ id: 'flux1-schnell-fp8', family: 'flux_schnell', modality: 'image', bytes: 17_236_328_572, installed: false, downloadable: false }] as ComfyUiStatus['suggestions'],
    }));
    await act(async () => { render(<ComfyUiCard t={t} />); });
    expect(screen.getByTestId('comfyui-suggestion-flux1-schnell-fp8')).toHaveTextContent('comfyui.notDownloadable');
    expect(screen.queryByTestId('comfyui-pull-flux1-schnell-fp8')).toBeNull();
  });
});

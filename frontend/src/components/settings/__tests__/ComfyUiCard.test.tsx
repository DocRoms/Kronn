import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { buildApiMock } from '../../../test/apiMock';
import type { ComfyUiStatus } from '../../../types/generated';

const { status, setSlots, pull, setSettings, setModelFamily } = vi.hoisted(() => ({
  status: vi.fn(), setSlots: vi.fn(), pull: vi.fn(), setSettings: vi.fn(), setModelFamily: vi.fn(),
}));
vi.mock('../../../lib/api', () => buildApiMock({
  comfyui: {
    status: status as never, setSlots: setSlots as never, pull: pull as never,
    setSettings: setSettings as never, setModelFamily: setModelFamily as never,
  },
}));
import { ComfyUiCard } from '../ComfyUiCard';

const t = (key: string, ...args: (string | number)[]) => args.length ? `${key}:${args.join(',')}` : key;

function online(patch: Partial<ComfyUiStatus> = {}): ComfyUiStatus {
  return {
    state: 'online', endpoint: 'http://127.0.0.1:8188', version: '0.42.1',
    models: [
      { folder: 'checkpoints', file: 'RealVisXL_V5.0_fp16.safetensors', family: 'sdxl', modality: 'image', chosen: false },
      { folder: 'checkpoints', file: 'flux1-schnell-fp8.safetensors', family: 'flux_schnell', modality: 'image', chosen: false },
      { folder: 'checkpoints', file: 'mystery.safetensors', family: null, modality: null, chosen: false },
      { folder: 'diffusion_models', file: 'wan2.1_t2v_1.3B_fp16.safetensors', family: 'wan21_t2v', modality: 'video', chosen: false },
    ],
    image_model: null, video_model: null, video_blocker: null,
    install_command: 'brew install --cask comfy', download_url: 'https://www.comfy.org/download',
    suggestions: [],
    install_dir: '/home/someone/ComfyUI-Installs', configured_endpoint: null, configured_install_dir: null,
    image_settings: {}, video_settings: {},
    family_defaults: [
      { family: 'sdxl', settings: { steps: 30, cfg: 5, sampler: 'dpmpp_sde', scheduler: 'karras' } },
      { family: 'wan21_t2v', settings: { steps: 30, cfg: 6, sampler: 'uni_pc', scheduler: 'simple' } },
    ],
    samplers: ['euler', 'dpmpp_sde', 'uni_pc'], schedulers: ['simple', 'karras'],
    ...patch,
  };
}

beforeEach(() => {
  status.mockReset();
  setSlots.mockReset();
  pull.mockReset();
  setSettings.mockReset();
  setModelFamily.mockReset();
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

  it('tells the media surfaces to re-read the local source after a save', async () => {
    status.mockResolvedValue(online());
    setSlots.mockResolvedValue({ image_model: 'RealVisXL_V5.0_fp16.safetensors', video_model: null });
    const announced = vi.fn();
    window.addEventListener('kronn:comfyui-changed', announced);
    await act(async () => { render(<ComfyUiCard t={t} />); });
    fireEvent.focus(screen.getByTestId('comfyui-slot-image'));
    await act(async () => { fireEvent.click(screen.getByRole('option', { name: /RealVisXL/ })); });
    await waitFor(() => expect(announced).toHaveBeenCalledTimes(1));
    window.removeEventListener('kronn:comfyui-changed', announced);
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

  it('runs a model Kronn does not recognise once the user picks its workflow', async () => {
    status.mockResolvedValue(online());
    setModelFamily.mockResolvedValue({});
    await act(async () => { render(<ComfyUiCard t={t} />); });
    const picker = screen.getByTestId('comfyui-family-mystery.safetensors');
    fireEvent.focus(picker);
    // A checkpoint only takes an image workflow.
    expect(screen.queryByRole('option', { name: 'comfyui.family.wan21_t2v' })).not.toBeInTheDocument();
    await act(async () => { fireEvent.click(screen.getByRole('option', { name: 'comfyui.family.sdxl' })); });
    await waitFor(() => expect(setModelFamily).toHaveBeenCalledWith({
      folder: 'checkpoints', file: 'mystery.safetensors', family: 'sdxl',
    }));
    await waitFor(() => expect(status).toHaveBeenCalledTimes(2));
  });

  it('lets another machine set its address and install folder, even when nothing is found', async () => {
    status.mockResolvedValue(online({ state: 'not_found', version: null, models: [] }));
    setSettings.mockResolvedValue({});
    await act(async () => { render(<ComfyUiCard t={t} />); });
    const endpoint = screen.getByLabelText('comfyui.settings.endpoint');
    expect(endpoint).toHaveAttribute('placeholder', 'http://127.0.0.1:8188');
    expect(screen.getByLabelText('comfyui.settings.installDir')).toHaveAttribute('placeholder', '/home/someone/ComfyUI-Installs');
    expect(screen.getByTestId('comfyui-settings-save')).toBeDisabled();
    fireEvent.change(endpoint, { target: { value: 'http://192.168.1.20:8000' } });
    fireEvent.change(screen.getByLabelText('comfyui.settings.installDir'), { target: { value: 'D:\\ComfyUI' } });
    await act(async () => { fireEvent.click(screen.getByTestId('comfyui-settings-save')); });
    expect(setSettings).toHaveBeenCalledWith({
      endpoint: 'http://192.168.1.20:8000',
      install_dir: 'D:\\ComfyUI',
      image_settings: { steps: null, cfg: null, sampler: null, scheduler: null },
      video_settings: { steps: null, cfg: null, sampler: null, scheduler: null },
    });
    await waitFor(() => expect(status).toHaveBeenCalledTimes(2));
  });

  it('shows the slot model defaults and saves per-slot overrides', async () => {
    status.mockResolvedValue(online({ image_model: 'RealVisXL_V5.0_fp16.safetensors', video_settings: { steps: 20 } }));
    setSettings.mockResolvedValue({});
    await act(async () => { render(<ComfyUiCard t={t} />); });
    const steps = screen.getByLabelText('comfyui.settings.steps', { selector: '#comfyui-steps-image' });
    expect(steps).toHaveAttribute('placeholder', 'comfyui.settings.default:30');
    expect(screen.getByLabelText('comfyui.settings.steps', { selector: '#comfyui-steps-video' })).toHaveValue(20);
    fireEvent.change(steps, { target: { value: '12' } });
    fireEvent.focus(screen.getByTestId('comfyui-sampler-image'));
    fireEvent.click(screen.getByRole('option', { name: 'euler' }));
    await act(async () => { fireEvent.click(screen.getByTestId('comfyui-settings-save')); });
    expect(setSettings).toHaveBeenCalledWith(expect.objectContaining({
      endpoint: null,
      install_dir: null,
      image_settings: { steps: 12, cfg: null, sampler: 'euler', scheduler: null },
      video_settings: { steps: 20, cfg: null, sampler: null, scheduler: null },
    }));
  });

  it('shows a refused setting instead of saving it silently', async () => {
    status.mockResolvedValue(online());
    setSettings.mockRejectedValue(new Error('ftp://box is not an http(s) address'));
    await act(async () => { render(<ComfyUiCard t={t} />); });
    fireEvent.change(screen.getByLabelText('comfyui.settings.endpoint'), { target: { value: 'ftp://box' } });
    await act(async () => { fireEvent.click(screen.getByTestId('comfyui-settings-save')); });
    expect(await screen.findByText('ftp://box is not an http(s) address')).toBeInTheDocument();
    expect(status).toHaveBeenCalledTimes(1);
  });
});

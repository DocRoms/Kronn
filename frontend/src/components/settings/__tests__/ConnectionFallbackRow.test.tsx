// KT-1706 — the image / video row of ComfyUI or of an HTTP connection:
// « Mode · [model ▾] → fallback ✕  + Add a fallback », kind-filtered, saved at
// once, and left as it was when the backend refuses.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { I18nProvider } from '../../../lib/I18nContext';
import { ConnectionFallbackRow } from '../ConnectionFallbackRow';
import { buildMediaOptions, COMFYUI_OWNER } from '../../../lib/mediaFallbackOptions';
import type { ConnectionChainView } from '../../../types/generated';

afterEach(cleanup);

const connections = [
  { id: 'conn-a', display_name: 'OpenRouter', image_model: 'flux-pro', video_model: 'seedance' },
  { id: 'conn-b', display_name: 'LiteLLM', image_model: null, video_model: 'veo' },
] as never;
const comfy = {
  models: [
    { folder: 'checkpoints', file: 'sdxl.safetensors', modality: 'image', family: 'sdxl' },
    { folder: 'diffusion_models', file: 'wan.safetensors', modality: 'video', family: 'wan21_t2v' },
    { folder: 'checkpoints', file: 'mystery.safetensors', modality: 'image', family: null },
  ],
  image_model: 'sdxl.safetensors',
  video_model: 'wan.safetensors',
} as never;

const renderRow = (
  props: Partial<React.ComponentProps<typeof ConnectionFallbackRow>> & { mode?: 'image' | 'video'; owner?: string },
) => {
  const onChange = props.onChange ?? vi.fn().mockResolvedValue(undefined);
  render(
    <I18nProvider>
      <ConnectionFallbackRow
        testId="row"
        mode={props.mode ?? 'image'}
        owner={props.owner ?? COMFYUI_OWNER}
        model={<span>sdxl.safetensors</span>}
        options={buildMediaOptions(connections, comfy)}
        view={props.view}
        onChange={onChange}
      />
    </I18nProvider>,
  );
  return onChange;
};

describe('buildMediaOptions', () => {
  it('lists the image and video model of each connection and the configured ones of ComfyUI', () => {
    const options = buildMediaOptions(connections, comfy);
    expect(options.map(o => `${o.connection}:${o.mode}:${o.model}`)).toEqual([
      'conn-a:image:flux-pro',
      'conn-a:video:seedance',
      'conn-b:video:veo',
      'local:comfyui:image:sdxl.safetensors',
      'local:comfyui:video:wan.safetensors',
    ]);
  });
});

describe('ConnectionFallbackRow', () => {
  it('shows the mode, the model picker and « no fallback » when the row has none', () => {
    renderRow({});
    expect(screen.getByTestId('row-model')).toHaveTextContent('sdxl.safetensors');
    expect(screen.queryByTestId('row-link-0')).not.toBeInTheDocument();
    expect(screen.getByTestId('row-add')).toBeInTheDocument();
  });

  it('offers only image models, of the other sources, and saves the pick at once', async () => {
    const onChange = renderRow({ owner: COMFYUI_OWNER, mode: 'image' });
    fireEvent.click(screen.getByTestId('row-add'));
    const menu = screen.getByTestId('row-menu');
    expect(within(menu).getAllByRole('group').map(group => group.getAttribute('aria-label'))).toEqual(['OpenRouter']);
    fireEvent.click(screen.getByTestId('row-menu-conn-a-image-flux-pro'));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith({ connection: 'conn-a', model: 'flux-pro' }));
  });

  it('keeps video and image apart, and never offers the source itself', () => {
    renderRow({ owner: 'conn-a', mode: 'video' });
    fireEvent.click(screen.getByTestId('row-add'));
    const menu = screen.getByTestId('row-menu');
    expect(within(menu).getAllByRole('group').map(group => group.getAttribute('aria-label')).sort()).toEqual(['LiteLLM', 'ComfyUI'].sort());
    expect(within(menu).queryByText('flux-pro')).not.toBeInTheDocument();
    expect(within(menu).queryByText('seedance')).not.toBeInTheDocument();
  });

  it('draws the fallback pill with its transport badge, the inherited one dashed, and removes it', async () => {
    const view: ConnectionChainView = {
      owner: COMFYUI_OWNER,
      mode: 'image',
      links: [{ connection: 'conn-a', model: 'flux-pro', transport: 'http' }],
      inherited: [{ connection: 'conn-b', model: 'img-2', transport: 'http' }],
    };
    const onChange = renderRow({ view });
    const pill = screen.getByTestId('row-link-0');
    expect(pill).toHaveTextContent('OpenRouter');
    expect(pill).toHaveTextContent('flux-pro');
    expect(within(pill).getByTestId('fallback-transport-http')).toBeInTheDocument();
    expect(screen.getByTestId('row-inherited-0')).toHaveAttribute('data-tone', 'inherited');
    fireEvent.click(screen.getByTestId('row-link-0-remove'));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith(null));
  });

  it('shows the refusal of the backend and leaves the row as it was', async () => {
    const onChange = vi.fn().mockRejectedValue(new Error('This fallback would loop: a → b → a.'));
    renderRow({ onChange });
    fireEvent.click(screen.getByTestId('row-add'));
    fireEvent.click(screen.getByTestId('row-menu-conn-a-image-flux-pro'));
    expect(await screen.findByTestId('row-refused')).toHaveTextContent('loop');
    // Nothing was stored: still no fallback, still offered.
    expect(screen.queryByTestId('row-link-0')).not.toBeInTheDocument();
    expect(screen.getByTestId('row-add')).not.toBeDisabled();
  });
});

// KT-1706 — what an image or video fallback can be: the image / video model of
// ComfyUI and of every named HTTP connection that has one. Pure functions, so the
// ComfyUI card and the connection cards offer the very same list.

import type { ComfyUiModel, FallbackMode } from '../types/generated';
import type { ExternalApiConnectionView } from './api';
import type { FallbackOption } from './fallbackChain';

/** The owner key of the local ComfyUI; any other owner is a connection's id. */
export const COMFYUI_OWNER = 'local:comfyui';

export interface ComfyMediaSource {
  /** The model files ComfyUI reports. */
  models: ComfyUiModel[];
  image_model: string | null;
  video_model: string | null;
}

const MODES: { mode: FallbackMode; field: 'economy_model' | 'default_model' | 'reasoning_model' | 'decision_model' | 'image_model' | 'video_model' }[] = [
  { mode: 'eco', field: 'economy_model' },
  { mode: 'standard', field: 'default_model' },
  { mode: 'advanced', field: 'reasoning_model' },
  { mode: 'decision', field: 'decision_model' },
  { mode: 'image', field: 'image_model' },
  { mode: 'video', field: 'video_model' },
];

/** Every media source and the model it would run: a connection's own slot, and the
 *  models ComfyUI holds for that kind (a model Kronn cannot run is not offered). */
export function buildMediaOptions(
  connections: (Pick<ExternalApiConnectionView, 'id' | 'display_name' | 'image_model' | 'video_model'> & Partial<Pick<ExternalApiConnectionView, 'economy_model' | 'default_model' | 'reasoning_model' | 'decision_model'>>)[],
  comfy: ComfyMediaSource | null,
  comfyName = 'ComfyUI',
): FallbackOption[] {
  const options: FallbackOption[] = [];
  for (const connection of connections) {
    for (const { mode, field } of MODES) {
      const model = connection[field]?.trim();
      if (model) options.push({ agent: 'Custom', connection: connection.id, name: connection.display_name, model, mode });
    }
  }
  if (comfy) {
    for (const { mode, field } of MODES) {
      if (field !== 'image_model' && field !== 'video_model') continue;
      const current = comfy[field];
      if (current) options.push({ agent: 'Custom', connection: COMFYUI_OWNER, name: comfyName, model: current, mode });
    }
  }
  return options;
}

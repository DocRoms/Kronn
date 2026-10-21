import type { SearchableSelectOption } from '../components/SearchableSelect';
import type { ComfyUiModel, MediaModality } from '../types/generated';

/** The models one slot may take: its kind only. A model Kronn has no measured
 *  workflow for stays listed, disabled, so the user sees why it is not offered. */
export function comfyUiSlotOptions(
  models: ComfyUiModel[],
  modality: MediaModality,
  t: (key: string, ...args: (string | number)[]) => string,
): SearchableSelectOption[] {
  const folder = modality === 'image' ? 'checkpoints' : 'diffusion_models';
  return models
    .filter(model => model.folder === folder)
    .filter(model => model.modality === null || model.modality === modality)
    .map(model => ({
      value: model.file,
      label: model.family ? model.file : `${model.file} — ${t('comfyui.unrecognised')}`,
      keywords: model.file,
      description: model.family ? t(`comfyui.family.${model.family}`) : t('comfyui.unrecognisedHint'),
      disabled: !model.family,
    }));
}


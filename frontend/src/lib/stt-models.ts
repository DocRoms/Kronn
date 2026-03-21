export interface SttModel {
  id: string;
  label: string;
  size: string;
  description: string;
}

export const STT_MODELS: SttModel[] = [
  { id: 'onnx-community/whisper-tiny',  label: 'Tiny',  size: '~40 MB',  description: 'Rapide, qualité basique' },
  { id: 'onnx-community/whisper-base',  label: 'Base',  size: '~140 MB', description: 'Bon compromis vitesse/qualité' },
  { id: 'onnx-community/whisper-small', label: 'Small', size: '~460 MB', description: 'Haute qualité, plus lent' },
];

export const DEFAULT_STT_MODEL = STT_MODELS[0].id;

export function getSttModelId(): string {
  try {
    const stored = localStorage.getItem('kronn:sttModel');
    if (stored && STT_MODELS.some(m => m.id === stored)) return stored;
  } catch { /* ignore */ }
  return DEFAULT_STT_MODEL;
}

export function setSttModelId(modelId: string) {
  localStorage.setItem('kronn:sttModel', modelId);
}

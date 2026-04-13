import { config as configApi } from './api';

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

/** Synchronous localStorage read — used for immediate render before the
 *  backend fetch lands. Falls back to the default when nothing is stored. */
export function getSttModelId(): string {
  try {
    const stored = localStorage.getItem('kronn:sttModel');
    if (stored && STT_MODELS.some(m => m.id === stored)) return stored;
  } catch { /* ignore */ }
  return DEFAULT_STT_MODEL;
}

/** Best-effort: reads from backend (source of truth) and falls back to
 *  localStorage on error. Use at app boot / settings mount to recover from
 *  a Tauri WebView2 localStorage wipe. */
export async function fetchSttModelId(): Promise<string> {
  try {
    const backend = await configApi.getSttModel();
    if (backend && STT_MODELS.some(m => m.id === backend)) {
      // Hydrate localStorage so the next sync read (pre-fetch) is correct.
      try { localStorage.setItem('kronn:sttModel', backend); } catch { /* ignore */ }
      return backend;
    }
  } catch { /* backend offline — fall back */ }
  return getSttModelId();
}

/** Sync setter — writes both localStorage (fast-path) and backend (durable).
 *  Backend write is fire-and-forget so the UI stays snappy; a failure is
 *  logged but doesn't block the caller. */
export function setSttModelId(modelId: string) {
  try { localStorage.setItem('kronn:sttModel', modelId); } catch { /* ignore */ }
  configApi.saveSttModel(modelId).catch(e => {
    console.warn('Failed to persist STT model to backend:', e);
  });
}

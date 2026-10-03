import type { SetupStatus } from '../types/generated';

export let RETRY_DELAY = 2000;
export function setRetryDelay(ms: number) { RETRY_DELAY = ms; }

export let STATUS_TIMEOUT_MS = 8000;
export function setStatusTimeout(ms: number) { STATUS_TIMEOUT_MS = ms; }

const SETUP_STATUS_CACHE_KEY = 'kronn:setup-status';

function isSetupStatus(value: unknown): value is SetupStatus {
  if (!value || typeof value !== 'object') return false;
  const status = value as Partial<SetupStatus>;
  return typeof status.is_first_run === 'boolean'
    && typeof status.current_step === 'string'
    && typeof status.scan_paths_set === 'boolean'
    && Array.isArray(status.agents_detected)
    && Array.isArray(status.repos_detected)
    && Array.isArray(status.scan_paths_explored)
    && (status.default_scan_path === null || typeof status.default_scan_path === 'string');
}

export function readCachedSetupStatus(storage: Pick<Storage, 'getItem'> = localStorage): SetupStatus | null {
  try {
    const cached = JSON.parse(storage.getItem(SETUP_STATUS_CACHE_KEY) ?? 'null') as unknown;
    return isSetupStatus(cached) ? cached : null;
  } catch {
    return null;
  }
}

export function cacheSetupStatus(
  status: SetupStatus,
  storage: Pick<Storage, 'setItem'> = localStorage,
): void {
  try {
    storage.setItem(SETUP_STATUS_CACHE_KEY, JSON.stringify(status));
  } catch {
    // Restricted/private browser storage must not make boot fail.
  }
}

export function clearCachedSetupStatus(storage: Pick<Storage, 'removeItem'> = localStorage): void {
  try {
    storage.removeItem(SETUP_STATUS_CACHE_KEY);
  } catch {
    // Restricted/private browser storage must not make reset fail.
  }
}

export function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) => setTimeout(() => reject(new Error('timeout')), ms)),
  ]);
}

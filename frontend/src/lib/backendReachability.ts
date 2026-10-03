// Shared signal between the API client, the backend status pill and data
// hooks. A failed request only asks for a health check; `/api/health` decides
// whether the backend is down, so one bad endpoint never raises the pill.

const SUSPECT_EVENT = 'kronn:backend-suspect';
const RECOVERED_EVENT = 'kronn:backend-recovered';

/** A request failed the way a stopped backend fails: check health now. */
export function reportBackendSuspect(): void {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(new Event(SUSPECT_EVENT));
}

/** The backend answers again after an outage: failed loads may retry. */
export function reportBackendRecovered(): void {
  if (typeof window === 'undefined') return;
  window.dispatchEvent(new Event(RECOVERED_EVENT));
}

export function onBackendSuspect(listener: () => void): () => void {
  window.addEventListener(SUSPECT_EVENT, listener);
  return () => window.removeEventListener(SUSPECT_EVENT, listener);
}

export function onBackendRecovered(listener: () => void): () => void {
  window.addEventListener(RECOVERED_EVENT, listener);
  return () => window.removeEventListener(RECOVERED_EVENT, listener);
}

/** Network errors and gateway answers without a JSON body (the dev proxy and
 *  nginx while the backend restarts); a real API error always carries JSON. */
export function looksLikeBackendDown(error: unknown, status?: number): boolean {
  if (error instanceof TypeError) return true;
  return status === 500 || status === 502 || status === 503 || status === 504;
}

export type BackendHealth = 'unknown' | 'up' | 'down';

let health: BackendHealth = 'unknown';
const healthListeners = new Set<() => void>();

/** Written by the status pill, the only component that polls `/api/health`. */
export function setBackendHealth(next: BackendHealth): void {
  if (health === next) return;
  health = next;
  for (const listener of [...healthListeners]) listener();
}

export function getBackendHealth(): BackendHealth {
  return health;
}

export function subscribeBackendHealth(listener: () => void): () => void {
  healthListeners.add(listener);
  return () => healthListeners.delete(listener);
}

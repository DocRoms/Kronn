export let RETRY_DELAY = 2000;
export function setRetryDelay(ms: number) { RETRY_DELAY = ms; }

export let STATUS_TIMEOUT_MS = 8000;
export function setStatusTimeout(ms: number) { STATUS_TIMEOUT_MS = ms; }

export function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return Promise.race([
    promise,
    new Promise<T>((_, reject) => setTimeout(() => reject(new Error('timeout')), ms)),
  ]);
}

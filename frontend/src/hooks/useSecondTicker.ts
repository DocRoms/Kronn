/**
 * One shared 1-second clock for every live duration on screen: however many
 * bubbles count, a single interval runs, and none while nobody listens.
 */
import { useSyncExternalStore } from 'react';

const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | undefined;
let now = Date.now();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (!timer) {
    now = Date.now();
    timer = setInterval(() => {
      now = Date.now();
      listeners.forEach(notify => notify());
    }, 1000);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && timer) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

const noop = () => () => {};
const snapshot = () => now;

/** The current time, refreshed every second while `active`. */
export function useSecondTicker(active: boolean): number {
  return useSyncExternalStore(active ? subscribe : noop, snapshot, snapshot);
}

/** Whether the shared clock is running; for tests. */
export function secondTickerRunning(): boolean {
  return timer !== undefined;
}

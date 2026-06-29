import '@testing-library/jest-dom/vitest';
import { configure } from '@testing-library/react';

// CI runs `vitest run --coverage` (v8 instrumentation) with heavy file
// parallelism on a shared runner. That slows React effect / microtask
// scheduling enough that the default 1000ms `waitFor` timeout can expire
// before a mount-effect assertion resolves — surfacing as rare, non-local
// flakes (e.g. DebugSection's "getLogs called on mount"). Raising the global
// async timeout gives slow CI runners headroom with ZERO cost on passing
// tests: `waitFor` returns as soon as its callback passes, so a higher ceiling
// only matters when the environment is genuinely slow.
configure({ asyncUtilTimeout: 5000 });

// localStorage polyfill for the test env. Node ≥22 ships an EXPERIMENTAL global
// `localStorage` that is inert unless `--localstorage-file` is passed, and it
// shadows happy-dom's storage — so under that Node every storage-backed module
// (i18n locale, theme/density, chat drafts, audit checkpoints, TTS prefs) and
// every spec that calls `localStorage.clear()` throws "Cannot read properties
// of undefined (reading 'getItem')". Install a deterministic in-memory Storage
// when the ambient one is missing/non-functional. No-op where happy-dom already
// provides a working store; `configurable`/`writable` so specs can still spy.
function makeMemoryStorage(): Storage {
  let store = new Map<string, string>();
  return {
    get length() { return store.size; },
    clear() { store = new Map(); },
    getItem(key: string) { return store.has(key) ? store.get(key)! : null; },
    key(i: number) { return Array.from(store.keys())[i] ?? null; },
    removeItem(key: string) { store.delete(key); },
    setItem(key: string, value: string) { store.set(String(key), String(value)); },
  } as Storage;
}
for (const name of ['localStorage', 'sessionStorage'] as const) {
  const cur = (globalThis as Record<string, unknown>)[name] as Storage | undefined;
  if (!cur || typeof cur.getItem !== 'function') {
    Object.defineProperty(globalThis, name, {
      value: makeMemoryStorage(),
      configurable: true,
      writable: true,
    });
  }
}

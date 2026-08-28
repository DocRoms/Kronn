import '@testing-library/jest-dom/vitest';
import { configure } from '@testing-library/react';

// Unit tests must never fall through to happy-dom's real HTTP client. A missing
// API mock previously opened requests to localhost:3000; Vitest then aborted
// those sockets while tearing workers down and intermittently exited 2 after all
// assertions had passed. Install a deterministic fail-closed boundary before
// every spec. Tests that exercise fetch explicitly replace it with `vi.stubGlobal`
// or a local spy, then restore this guard.
const unexpectedFetch: typeof fetch = (input) => {
  const target = input instanceof Request ? input.url : String(input);
  throw new Error(
    `[test-network-guard] Unexpected unmocked fetch to ${target}. `
      + 'Stub globalThis.fetch or mock the API module in this test.',
  );
};
Object.defineProperty(globalThis, 'fetch', {
  value: unexpectedFetch,
  configurable: true,
  writable: true,
});

// CI runs `vitest run --coverage` (v8 instrumentation) with heavy file
// parallelism on a shared runner. That slows React effect / microtask
// scheduling enough that the default 1000ms `waitFor` timeout can expire
// before a mount-effect assertion resolves — surfacing as rare, non-local
// flakes (e.g. DebugSection's "getLogs called on mount"). Raising the global
// async timeout gives slow CI runners headroom with ZERO cost on passing
// tests: `waitFor` returns as soon as its callback passes, so a higher ceiling
// only matters when the environment is genuinely slow.
configure({ asyncUtilTimeout: 5000 });

// Deterministic storage for the test env. Node >= 22 ships EXPERIMENTAL global
// storage accessors that are inert without `--localstorage-file` and shadow
// happy-dom's stores. Install our in-memory implementation without reading the
// ambient accessors; `configurable`/`writable` lets specs continue to spy. This
// lightweight implementation supports the Storage methods used by the app,
// not named-property access such as `localStorage['key']`.
// 0.8.11 (D10) — getUILocale() now follows the browser's language when no
// locale is stored. The test runner's navigator.language is 'en-US', which would
// flip every French-asserting component test to English. Pin the browser locale
// to French in the test env so those assertions stay deterministic (and the
// "default French" locale tests keep their intent). Robust against
// localStorage.clear() (unlike seeding storage). Real browser detection is
// covered by src/lib/__tests__/i18n.test.ts via detectBrowserLocale().
try {
  Object.defineProperty(navigator, 'language', { value: 'fr-FR', configurable: true });
  Object.defineProperty(navigator, 'languages', { value: ['fr-FR', 'fr'], configurable: true });
} catch { /* some envs freeze navigator — best effort */ }

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
// Install both stores unconditionally. On Node >= 22, merely reading the
// ambient `globalThis.localStorage` accessor emits an ExperimentalWarning
// unless `--localstorage-file` is configured. Vitest forwards that warning
// through the worker RPC; under a heavily parallel suite the worker can be
// torn down while `onUserConsoleLog` is still pending. Defining the test
// stores without probing the ambient accessor avoids the warning entirely
// and gives every worker the same deterministic storage implementation.
for (const name of ['localStorage', 'sessionStorage'] as const) {
  Object.defineProperty(globalThis, name, {
    value: makeMemoryStorage(),
    configurable: true,
    writable: true,
  });
}

// Unit tests exercise the synchronous `t()` API directly and intentionally
// validate every shipped dictionary. Production preloads only the active locale
// in main.tsx; the test harness loads all chunks once before specs start.
const { loadLocale } = await import('../lib/i18n');
await Promise.all([loadLocale('fr'), loadLocale('en'), loadLocale('es'), loadLocale('zh')]);

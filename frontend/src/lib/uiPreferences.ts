/**
 * Server mirror of the interface preferences kept in localStorage (KT-972).
 *
 * localStorage is scoped to the page origin, and the desktop's origin is
 * `http://127.0.0.1:<port>`: a port fallback, a first launch or a restart on
 * another port starts from an empty storage. The backend keeps a copy
 * (`/api/ui-preferences`) that a fresh origin is hydrated from before the
 * first render, and every change is written through after a short delay.
 *
 * Only durable, user-visible choices are synced. Not synced, on purpose:
 * drafts and the message outbox (unsent text belongs to the device that
 * typed it; sharing an outbox could send it twice), caches the server already
 * owns (setup status, unread counts), per-session navigation, the UI locale
 * and voices (already server-side) and anything secret-adjacent such as the
 * secret-input reveal toggle.
 */
import { config as configApi } from './api';
import { safeGetItem, safeSetItem, setStorageWriteListener } from './safeStorage';
import { isTauriRuntime } from './tauri';

type Values = Record<string, string>;

export const SYNCED_KEYS: readonly string[] = [
  'kronn:theme',
  'kronn:unlockedThemes',
  'kronn:layoutDensity',
  'kronn:tour-progress:v1',
  'kronn:update-dismissed-version',
  'kronn:new-discussion:default-project',
  'kronn:ttsEnabled',
  'kronn:showDiscussionNotes',
  'kronn:searchScope',
  'kronn:discHeaderDetails',
  'kronn:projectDetailView',
  'kronn:ollamaDownloadOpen',
  'kronn:sidebarCollapsed',
  'kronn:plugins:sidebarCollapsed',
  'kronn:automation:sidebarCollapsed',
  'kronn:pages:sidebarCollapsed',
  'kronn:project-sidebar-collapsed-sections',
  'kronn:discSidebarSectionsV2',
  'kronn:discCollapsedGroups',
  'kronn:wfCollapsedGroups',
  'kronn:mcpCollapsedGroups',
  'kronn:planningCollapsedSections',
  'kronn:automationCollapsedSections',
  'kronn:pageCollapsedSections',
  'kronn:automationGroupBy',
  'kronn:mcpSort',
  'kronn:automationSkillFavorites',
  'kronn:fullAccessNoticeDismissed',
  'kronn:runRetentionBannerDismissed',
];

export const SYNCED_PREFIXES: readonly string[] = ['kronn:collection-favorites:'];

/** The participant id a human uses to join rooms by code. A desktop install
 *  keeps one across origins; a browser keeps its own, so it is never adopted
 *  from (or written to) the server by a browser. */
export const DESKTOP_ONLY_KEYS: readonly string[] = ['kronn:webSessionId'];

/** Per-origin marker: absent means this origin's storage was never hydrated. */
export const HYDRATED_MARKER = 'kronn:ui-preferences:hydrated';

const WRITE_DELAY_MS = 400;
const CHECK_INTERVAL_MS = 5_000;
const BOOT_TIMEOUT_MS = 1_500;
const WRITE_TIMEOUT_MS = 2_000;
/** Below the backend's 64 KiB cap, so an oversized snapshot is not retried forever. */
const MAX_PAYLOAD_BYTES = 60_000;

export interface UiPreferencesTransport {
  get(): Promise<Values>;
  put(values: Values): Promise<void>;
}

interface Options {
  transport?: UiPreferencesTransport;
  isDesktop?: () => boolean;
}

export function isSyncedKey(key: string, desktop: boolean): boolean {
  if (SYNCED_KEYS.includes(key) || SYNCED_PREFIXES.some(prefix => key.startsWith(prefix))) return true;
  return desktop && DESKTOP_ONLY_KEYS.includes(key);
}

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('ui preferences timeout')), ms);
    promise.then(
      value => { clearTimeout(timer); resolve(value); },
      error => { clearTimeout(timer); reject(error); },
    );
  });
}

function stableJson(values: Values): string {
  return JSON.stringify(Object.keys(values).sort().map(key => [key, values[key]]));
}

export function createUiPreferencesSync(options: Options = {}) {
  const transport: UiPreferencesTransport = options.transport ?? {
    get: () => configApi.getUiPreferences(),
    put: values => configApi.saveUiPreferences(values),
  };
  const isDesktop = options.isDesktop ?? isTauriRuntime;

  /** Last map known to be on the server; null until a read succeeded. */
  let server: Values | null = null;
  let writeTimer: ReturnType<typeof setTimeout> | null = null;
  let hydrating: Promise<boolean> | null = null;

  /** null when storage cannot be read: then nothing is pushed, since an
   *  empty snapshot would erase the server copy. */
  const snapshot = (): Values | null => {
    const desktop = isDesktop();
    const values: Values = {};
    try {
      const store = localStorage;
      for (let index = 0; index < store.length; index += 1) {
        const key = store.key(index);
        if (key === null || !isSyncedKey(key, desktop)) continue;
        const value = store.getItem(key);
        if (value !== null) values[key] = value;
      }
    } catch {
      return null;
    }
    return values;
  };

  /** Keys this client does not manage (another client's, or a newer
   *  frontend's) are carried over, since a PUT replaces the whole map. */
  const payload = (local: Values): Values => {
    const desktop = isDesktop();
    const kept: Values = {};
    for (const [key, value] of Object.entries(server ?? {})) {
      if (!isSyncedKey(key, desktop)) kept[key] = value;
    }
    return { ...kept, ...local };
  };

  const write = (key: string, value: string) => {
    try {
      localStorage.setItem(key, value);
    } catch {
      // Storage full or blocked: the value stays server-side for next time.
    }
  };

  const hydrateOnce = async (timeoutMs: number): Promise<boolean> => {
    const fresh = safeGetItem(HYDRATED_MARKER) === null;
    let remote: Values;
    try {
      remote = await withTimeout(transport.get(), timeoutMs);
    } catch {
      return false;
    }
    if (!remote || typeof remote !== 'object') return false;
    const desktop = isDesktop();
    const present = snapshot() ?? {};
    for (const [key, value] of Object.entries(remote)) {
      if (typeof value !== 'string' || !isSyncedKey(key, desktop)) continue;
      // A fresh origin takes the server's copy; an origin with its own
      // history only fills what it lacks.
      if (fresh || !(key in present)) write(key, value);
    }
    server = { ...remote };
    safeSetItem(HYDRATED_MARKER, '1');
    return true;
  };

  const hydrate = (timeoutMs: number): Promise<boolean> => {
    hydrating ??= hydrateOnce(timeoutMs).finally(() => { hydrating = null; });
    return hydrating;
  };

  /** Push the snapshot if it differs from the server copy. Never throws. */
  const flush = async (): Promise<void> => {
    if (writeTimer !== null) {
      clearTimeout(writeTimer);
      writeTimer = null;
    }
    // Writing before a successful read could replace the server's copy with
    // a fresh origin's defaults.
    if (server === null && !(await hydrate(BOOT_TIMEOUT_MS))) return;
    const local = snapshot();
    if (local === null) return;
    const next = payload(local);
    if (server !== null && stableJson(next) === stableJson(server)) return;
    if (JSON.stringify(next).length > MAX_PAYLOAD_BYTES) return;
    try {
      await withTimeout(transport.put(next), WRITE_TIMEOUT_MS);
      server = next;
    } catch {
      // Server unavailable: retried at the next change or check.
    }
  };

  const schedule = () => {
    if (writeTimer !== null) clearTimeout(writeTimer);
    writeTimer = setTimeout(() => { void flush(); }, WRITE_DELAY_MS);
  };

  /** Before the first render: blocks (bounded) only on a never-hydrated origin. */
  const boot = async (timeoutMs = BOOT_TIMEOUT_MS): Promise<void> => {
    try {
      if (safeGetItem(HYDRATED_MARKER) === null) {
        await hydrate(timeoutMs);
      } else {
        void hydrate(timeoutMs);
      }
    } catch {
      // localStorage alone keeps working.
    }
  };

  /** Write-through: debounced on guarded writes, plus a periodic check for
   *  writers that still use localStorage directly. */
  const start = () => {
    const desktop = isDesktop();
    setStorageWriteListener(key => { if (isSyncedKey(key, desktop)) schedule(); });
    const interval = setInterval(() => { void flush(); }, CHECK_INTERVAL_MS);
    const onHidden = () => { if (document.visibilityState === 'hidden') void flush(); };
    document.addEventListener('visibilitychange', onHidden);
    return () => {
      setStorageWriteListener(null);
      clearInterval(interval);
      document.removeEventListener('visibilitychange', onHidden);
      if (writeTimer !== null) clearTimeout(writeTimer);
      writeTimer = null;
    };
  };

  return { boot, flush, start, snapshot };
}

const shared = createUiPreferencesSync();

export const bootUiPreferences = shared.boot;
export const startUiPreferencesSync = shared.start;
/** Await before anything that reloads the app (desktop restart). */
export const flushUiPreferences = shared.flush;

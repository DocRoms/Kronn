import { createContext, useContext, useState, useCallback, useEffect, type ReactNode } from 'react';
import { themes as themesApi, type UnlockedItem } from './api';

/** Theme modes.
 *
 *  `system` resolves to light/dark from `prefers-color-scheme`.
 *  `light` and `dark` are always available.
 *  `matrix`, `sakura`, `gotham` are secret themes — the picker only
 *  shows them after the user has successfully submitted their unlock
 *  code via POST /api/themes/unlock. Codes are stored server-side
 *  (built-in hashes or config.toml) and never appear in this bundle.
 */
export type ThemeMode = 'system' | 'light' | 'dark' | 'matrix' | 'sakura' | 'gotham';

/** Themes that require unlock. Keep in sync with tokens.css. A theme
 *  name absent from this set is considered "always available". */
const SECRET_THEMES: ReadonlySet<ThemeMode> = new Set<ThemeMode>(['matrix', 'sakura', 'gotham']);

interface ThemeContextValue {
  theme: ThemeMode;
  setTheme: (t: ThemeMode) => void;
  /** List of unlocked secret theme names (subset of SECRET_THEMES). */
  unlockedThemes: ThemeMode[];
  /** Submit a code to the backend. The server returns an array of
   *  `{ kind, name }` unlocks (bundle codes return multiple). Theme
   *  unlocks are appended to `unlockedThemes` and persisted locally;
   *  profile unlocks are already persisted server-side by the endpoint
   *  and are bubbled up here unchanged so the caller can refetch
   *  `/api/profiles`. Failure throws. */
  unlockTheme: (code: string) => Promise<UnlockedItem[]>;
}

const ThemeContext = createContext<ThemeContextValue>({
  theme: 'system',
  setTheme: () => {},
  unlockedThemes: [],
  unlockTheme: async () => { throw new Error('ThemeProvider not mounted'); },
});

const STORAGE_KEY = 'kronn:theme';
const UNLOCKED_KEY = 'kronn:unlockedThemes';

const isValidTheme = (s: unknown): s is ThemeMode =>
  s === 'system' || s === 'light' || s === 'dark' ||
  s === 'matrix' || s === 'sakura' || s === 'gotham';

function loadUnlocked(): ThemeMode[] {
  try {
    const raw = localStorage.getItem(UNLOCKED_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    // Keep only values that are both valid ThemeMode AND in the secret set
    // — defensive against someone tampering with localStorage to grant
    // themselves `light` (which doesn't need unlock anyway) or a typo like
    // `matrixx`. The picker ignores unknown entries either way.
    return parsed.filter((x): x is ThemeMode => isValidTheme(x) && SECRET_THEMES.has(x));
  } catch {
    return [];
  }
}

function saveUnlocked(themes: ThemeMode[]) {
  try {
    localStorage.setItem(UNLOCKED_KEY, JSON.stringify(themes));
  } catch { /* noop */ }
}

function getInitialTheme(unlocked: ThemeMode[]): ThemeMode {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isValidTheme(stored)) {
      // If the stored theme is a secret one but no longer unlocked
      // (fresh localStorage wipe on another browser, user cleared it),
      // degrade gracefully to system rather than applying a theme the
      // user can't reproduce.
      if (SECRET_THEMES.has(stored) && !unlocked.includes(stored)) {
        return 'system';
      }
      return stored;
    }
  } catch { /* SSR / restricted storage */ }
  return 'system';
}

function resolveTheme(mode: ThemeMode): Exclude<ThemeMode, 'system'> {
  if (mode !== 'system') return mode;
  return window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
}

function applyTheme(mode: ThemeMode) {
  const resolved = resolveTheme(mode);
  document.documentElement.setAttribute('data-theme', resolved);
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [unlockedThemes, setUnlockedThemes] = useState<ThemeMode[]>(loadUnlocked);
  const [theme, setThemeState] = useState<ThemeMode>(() => getInitialTheme(loadUnlocked()));

  const setTheme = useCallback((t: ThemeMode) => {
    // Gate on unlocked list — if a caller tries to set a secret theme
    // that isn't unlocked, silently fall back to `dark` (and don't
    // persist the attempt). Prevents hot-swapping via devtools past the
    // server-side check.
    if (SECRET_THEMES.has(t) && !unlockedThemes.includes(t)) {
      return;
    }
    try { localStorage.setItem(STORAGE_KEY, t); } catch { /* noop */ }
    setThemeState(t);
  }, [unlockedThemes]);

  const unlockTheme = useCallback(async (code: string) => {
    // Backend validates and returns an array — bundle codes yield
    // multiple unlocks (e.g. Batman = profile + theme). We persist
    // every theme unlock locally (dedup against the current set) and
    // hand the full list back to the caller so it can refetch
    // profile-dependent state for profile unlocks.
    const { unlocks } = await themesApi.unlock(code);

    const themeNames = unlocks
      .filter((u): u is UnlockedItem & { kind: 'theme' } => u.kind === 'theme')
      .map(u => u.name)
      .filter((n): n is ThemeMode => isValidTheme(n) && SECRET_THEMES.has(n as ThemeMode));

    // Reject if the server returned something with zero known entries —
    // protects against a config/bundle drift (e.g. operator added a
    // theme code on the server but the bundle lacks the CSS block).
    const hasProfile = unlocks.some(u => u.kind === 'profile');
    if (themeNames.length === 0 && !hasProfile) {
      throw new Error('Unknown unlock payload');
    }

    if (themeNames.length > 0) {
      setUnlockedThemes(prev => {
        const toAdd = themeNames.filter(n => !prev.includes(n));
        if (toAdd.length === 0) return prev;
        const next = [...prev, ...toAdd];
        saveUnlocked(next);
        return next;
      });
    }

    return unlocks;
  }, []);

  // Apply theme on mount and when it changes
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  // Listen for OS theme changes when in 'system' mode
  useEffect(() => {
    if (theme !== 'system') return;
    const mql = window.matchMedia('(prefers-color-scheme: light)');
    const handler = () => applyTheme('system');
    mql.addEventListener('change', handler);
    return () => mql.removeEventListener('change', handler);
  }, [theme]);

  return (
    <ThemeContext.Provider value={{ theme, setTheme, unlockedThemes, unlockTheme }}>
      {children}
    </ThemeContext.Provider>
  );
}

export function useTheme() {
  return useContext(ThemeContext);
}

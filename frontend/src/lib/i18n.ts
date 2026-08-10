// ─── Kronn i18n — lightweight translation system ───────────────────────────
// UI language (stored in localStorage) is separate from agent output language (stored in backend config).

export type UILocale = 'fr' | 'en' | 'es' | 'zh';

export const UI_LOCALES: { code: UILocale; label: string; flag: string }[] = [
  { code: 'fr', label: 'Français', flag: '🇫🇷' },
  { code: 'en', label: 'English', flag: '🇬🇧' },
  { code: 'es', label: 'Español', flag: '🇪🇸' },
  { code: 'zh', label: '简体中文', flag: '🇨🇳' },
];

const STORAGE_KEY = 'kronn:ui-locale';

export function getUILocale(): UILocale {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (isUILocale(stored)) return stored;
  } catch { /* no localStorage available — fall back to browser detection */ }
  return detectBrowserLocale();
}

/** Map the browser's language preferences to a supported UI locale, else 'en'. */
export function detectBrowserLocale(): UILocale {
  try {
    const langs =
      typeof navigator !== 'undefined'
        ? (navigator.languages && navigator.languages.length
            ? navigator.languages
            : [navigator.language]).filter(Boolean)
        : [];
    for (const l of langs) {
      const base = String(l).toLowerCase().split('-')[0];
      if (isUILocale(base)) return base;
    }
  } catch { /* navigator unavailable */ }
  return 'en';
}

export function setUILocale(locale: UILocale) {
  try {
    localStorage.setItem(STORAGE_KEY, locale);
  } catch { /* no localStorage available — locale just won't persist */ }
}

// ─── Lazy translation dictionaries ────────────────────────

export type TranslationDict = Record<string, string>;

type LocaleModule = { default: TranslationDict };
type LocaleLoader = () => Promise<LocaleModule>;

const defaultLocaleLoaders: Record<UILocale, LocaleLoader> = {
  fr: () => import('./i18n/locales/fr'),
  en: () => import('./i18n/locales/en'),
  es: () => import('./i18n/locales/es'),
  zh: () => import('./i18n/locales/zh'),
};

let localeLoaders = defaultLocaleLoaders;
const loadedDictionaries: Partial<Record<UILocale, TranslationDict>> = {};
const pendingLoads: Partial<Record<UILocale, Promise<TranslationDict>>> = {};

export function isUILocale(value: unknown): value is UILocale {
  return value === 'fr' || value === 'en' || value === 'es' || value === 'zh';
}

/** Load one locale chunk once. A rejected chunk is evicted so a retry can succeed. */
export function loadLocale(locale: UILocale): Promise<TranslationDict> {
  const loaded = loadedDictionaries[locale];
  if (loaded) return Promise.resolve(loaded);
  const pending = pendingLoads[locale];
  if (pending) return pending;

  const request = localeLoaders[locale]().then(({ default: dictionary }) => {
    loadedDictionaries[locale] = dictionary;
    delete pendingLoads[locale];
    return dictionary;
  }, error => {
    delete pendingLoads[locale];
    throw error;
  });
  pendingLoads[locale] = request;
  return request;
}

export function isLocaleLoaded(locale: UILocale): boolean {
  return loadedDictionaries[locale] !== undefined;
}

/** Get a translated string with optional {0}, {1}... interpolation. */
export function t(locale: UILocale, key: string, ...args: (string | number)[]): string {
  const dict = loadedDictionaries[locale];
  const fallback = loadedDictionaries.fr;
  let str = dict?.[key] ?? fallback?.[key] ?? key;
  for (let i = 0; i < args.length; i++) {
    str = str.replace(`{${i}}`, String(args[i]));
  }
  return str;
}

/** Test-only seam for loader failure/race coverage without eager locale imports. */
export function __setLocaleLoadersForTests(loaders: Record<UILocale, LocaleLoader>): () => void {
  if (!import.meta.env.MODE.includes('test')) {
    throw new Error('Locale loader overrides are test-only');
  }
  localeLoaders = loaders;
  for (const { code: locale } of UI_LOCALES) {
    delete loadedDictionaries[locale];
    delete pendingLoads[locale];
  }
  return () => {
    localeLoaders = defaultLocaleLoaders;
    for (const { code: locale } of UI_LOCALES) {
      delete loadedDictionaries[locale];
      delete pendingLoads[locale];
    }
  };
}

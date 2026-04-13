import { createContext, useContext, useState, useCallback, useEffect, type ReactNode } from 'react';
import { type UILocale, getUILocale, setUILocale as persistLocale, t } from './i18n';
import { config as configApi } from './api';

interface I18nContextValue {
  locale: UILocale;
  setLocale: (l: UILocale) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

const I18nContext = createContext<I18nContextValue>({
  locale: 'fr',
  setLocale: () => {},
  t: (key) => key,
});

const isValidLocale = (s: unknown): s is UILocale =>
  s === 'fr' || s === 'en' || s === 'es';

export function I18nProvider({ children }: { children: ReactNode }) {
  // Initial render: localStorage wins over "fr" default for snappy first paint.
  // The backend fetch below then corrects the value if the two disagree —
  // which is the scenario that bit Marie on Tauri Windows (WebView2 wiped
  // localStorage, so getUILocale() returned 'fr' even though backend had 'en').
  const [locale, setLocaleState] = useState<UILocale>(getUILocale);

  // Fetch the backend-stored UI locale once at mount and adopt it if it
  // differs from what localStorage returned. localStorage is also updated
  // so the next mount starts with the right value even before the fetch.
  useEffect(() => {
    let cancelled = false;
    configApi.getUiLanguage()
      .then(backendLocale => {
        if (cancelled) return;
        if (isValidLocale(backendLocale) && backendLocale !== locale) {
          persistLocale(backendLocale);
          setLocaleState(backendLocale);
        }
      })
      .catch(() => {
        // Backend unreachable (offline setup, first boot) → keep localStorage
        // value. No toast — this path is silent by design.
      });
    return () => { cancelled = true; };
    // Intentional: fetch ONCE at mount, don't re-fetch when `locale` flips.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const setLocale = useCallback((l: UILocale) => {
    // Write both: localStorage for immediate re-render + fast reload,
    // backend for cross-reboot persistence (survives WebView2 wipes).
    persistLocale(l);
    setLocaleState(l);
    configApi.saveUiLanguage(l).catch(e => {
      console.warn('Failed to persist UI locale to backend:', e);
    });
  }, []);

  useEffect(() => {
    document.documentElement.lang = locale;
  }, [locale]);

  const translate = useCallback((key: string, ...args: (string | number)[]) => {
    return t(locale, key, ...args);
  }, [locale]);

  return (
    <I18nContext.Provider value={{ locale, setLocale, t: translate }}>
      {children}
    </I18nContext.Provider>
  );
}

export function useT() {
  return useContext(I18nContext);
}

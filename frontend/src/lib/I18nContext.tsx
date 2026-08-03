import { createContext, useContext, useState, useCallback, useEffect, useRef, type ReactNode } from 'react';
import {
  type UILocale,
  getUILocale,
  isUILocale,
  loadLocale,
  setUILocale as persistLocale,
  t,
} from './i18n';
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

export function I18nProvider({ children }: { children: ReactNode }) {
  // Initial render: localStorage wins over "fr" default for snappy first paint.
  // The backend fetch below then corrects the value if the two disagree —
  // which is the scenario that bit Marie on Tauri Windows (WebView2 wiped
  // localStorage, so getUILocale() returned 'fr' even though backend had 'en').
  const [locale, setLocaleState] = useState<UILocale>(getUILocale);
  const localeRequestRef = useRef(0);

  // Fetch the backend-stored UI locale once at mount and adopt it if it
  // differs from what localStorage returned. localStorage is also updated
  // so the next mount starts with the right value even before the fetch.
  useEffect(() => {
    let cancelled = false;
    const requestId = ++localeRequestRef.current;
    configApi.getUiLanguage()
      .then(async backendLocale => {
        if (cancelled || requestId !== localeRequestRef.current) return;
        if (isUILocale(backendLocale) && backendLocale !== locale) {
          await loadLocale(backendLocale);
          if (cancelled || requestId !== localeRequestRef.current) return;
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
    const requestId = ++localeRequestRef.current;
    void loadLocale(l).then(() => {
      if (requestId !== localeRequestRef.current) return;
      persistLocale(l);
      setLocaleState(l);
      return configApi.saveUiLanguage(l);
    }).catch(e => {
      if (requestId !== localeRequestRef.current) return;
      console.warn('Failed to load or persist UI locale:', e);
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

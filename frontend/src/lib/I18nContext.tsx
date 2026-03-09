import { createContext, useContext, useState, useCallback, type ReactNode } from 'react';
import { type UILocale, getUILocale, setUILocale as persistLocale, t } from './i18n';

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
  const [locale, setLocaleState] = useState<UILocale>(getUILocale);

  const setLocale = useCallback((l: UILocale) => {
    persistLocale(l);
    setLocaleState(l);
  }, []);

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

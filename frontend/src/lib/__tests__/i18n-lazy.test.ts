import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  __setLocaleLoadersForTests,
  isLocaleLoaded,
  loadLocale,
  t,
  type TranslationDict,
  type UILocale,
} from '../i18n';

const restorers: Array<() => void> = [];

function install(loaders: Record<UILocale, () => Promise<{ default: TranslationDict }>>) {
  const restore = __setLocaleLoadersForTests(loaders);
  restorers.push(restore);
}

afterEach(async () => {
  while (restorers.length > 0) restorers.pop()?.();
  await Promise.all([loadLocale('fr'), loadLocale('en'), loadLocale('es')]);
});

describe('lazy locale loading', () => {
  it('deduplicates concurrent requests for the same locale', async () => {
    let resolve!: (module: { default: TranslationDict }) => void;
    const enLoader = vi.fn(() => new Promise<{ default: TranslationDict }>(done => {
      resolve = done;
    }));
    install({
      fr: async () => ({ default: {} }),
      en: enLoader,
      es: async () => ({ default: {} }),
    });

    const first = loadLocale('en');
    const second = loadLocale('en');
    expect(first).toBe(second);
    expect(enLoader).toHaveBeenCalledTimes(1);

    resolve({ default: { 'nav.projects': 'Projects' } });
    await expect(first).resolves.toEqual({ 'nav.projects': 'Projects' });
    expect(isLocaleLoaded('en')).toBe(true);
    expect(t('en', 'nav.projects')).toBe('Projects');
  });

  it('evicts a rejected request so the locale can be retried', async () => {
    const enLoader = vi.fn()
      .mockRejectedValueOnce(new Error('chunk unavailable'))
      .mockResolvedValueOnce({ default: { 'nav.projects': 'Projects' } });
    install({
      fr: async () => ({ default: {} }),
      en: enLoader,
      es: async () => ({ default: {} }),
    });

    await expect(loadLocale('en')).rejects.toThrow('chunk unavailable');
    expect(isLocaleLoaded('en')).toBe(false);
    await expect(loadLocale('en')).resolves.toEqual({ 'nav.projects': 'Projects' });
    expect(enLoader).toHaveBeenCalledTimes(2);
  });
});

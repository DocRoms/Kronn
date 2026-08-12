import { describe, expect, it, vi } from 'vitest';
import { loadInitialLocale, renderBootstrapFailure } from '../bootstrapLocale';
import type { TranslationDict, UILocale } from '../i18n';

describe('locale bootstrap fallback', () => {
  it('falls back to English and persists it when the preferred chunk fails', async () => {
    const loader = vi.fn(async (locale: UILocale): Promise<TranslationDict> => {
      if (locale === 'fr') throw new Error('missing fr chunk');
      return { 'nav.projects': 'Projects' };
    });
    const persist = vi.fn();

    await expect(loadInitialLocale('fr', loader, persist)).resolves.toBe('en');
    expect(loader.mock.calls.map(([locale]) => locale)).toEqual(['fr', 'en']);
    expect(persist).toHaveBeenCalledWith('en');
  });

  it('propagates failure when English itself cannot load', async () => {
    const loader = vi.fn(async () => {
      throw new Error('all chunks unavailable');
    });
    await expect(loadInitialLocale('en', loader)).rejects.toThrow('all chunks unavailable');
  });

  it('renders a visible retry action when no locale can bootstrap', () => {
    const root = document.createElement('div');
    const reload = vi.fn();
    renderBootstrapFailure(root, reload);

    expect(root.querySelector('[role="alert"]')?.textContent).toContain('could not start');
    const button = root.querySelector('button');
    expect(button?.textContent).toBe('Retry');
    button?.click();
    expect(reload).toHaveBeenCalledTimes(1);
  });

  it('renders native startup details as text without interpreting markup', () => {
    const root = document.createElement('div');
    renderBootstrapFailure(root, vi.fn(), 'Data lock held\n<script>unsafe()</script>');

    expect(root.querySelector('p')?.textContent).toBe(
      'Data lock held\n<script>unsafe()</script>',
    );
    expect(root.querySelector('script')).toBeNull();
  });

});

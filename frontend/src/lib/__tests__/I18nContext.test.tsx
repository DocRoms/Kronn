import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { I18nProvider, useT } from '../I18nContext';

// Test component that displays locale and a translation
function TestConsumer() {
  const { locale, setLocale, t } = useT();
  return (
    <div>
      <span data-testid="locale">{locale}</span>
      <span data-testid="translation">{t('nav.projects')}</span>
      <button data-testid="switch-en" onClick={() => setLocale('en')}>EN</button>
      <button data-testid="switch-es" onClick={() => setLocale('es')}>ES</button>
    </div>
  );
}

describe('I18nContext', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it('provides default French locale', () => {
    render(
      <I18nProvider>
        <TestConsumer />
      </I18nProvider>
    );

    expect(screen.getByTestId('locale')).toHaveTextContent('fr');
    expect(screen.getByTestId('translation')).toHaveTextContent('Projets');
  });

  it('switches locale and updates translations', async () => {
    const user = userEvent.setup();
    render(
      <I18nProvider>
        <TestConsumer />
      </I18nProvider>
    );

    await user.click(screen.getByTestId('switch-en'));

    expect(screen.getByTestId('locale')).toHaveTextContent('en');
    expect(screen.getByTestId('translation')).toHaveTextContent('Projects');
  });

  it('persists locale to localStorage', async () => {
    const user = userEvent.setup();
    render(
      <I18nProvider>
        <TestConsumer />
      </I18nProvider>
    );

    await user.click(screen.getByTestId('switch-es'));

    expect(localStorage.getItem('kronn:ui-locale')).toBe('es');
    expect(screen.getByTestId('translation')).toHaveTextContent('Proyectos');
  });

  it('restores locale from localStorage', () => {
    localStorage.setItem('kronn:ui-locale', 'en');

    render(
      <I18nProvider>
        <TestConsumer />
      </I18nProvider>
    );

    expect(screen.getByTestId('locale')).toHaveTextContent('en');
    expect(screen.getByTestId('translation')).toHaveTextContent('Projects');
  });
});

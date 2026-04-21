import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, act } from '@testing-library/react';
import { ThemeProvider } from '../../lib/ThemeContext';
import { ThemeEffects } from '../ThemeEffects';

/** We control theme from the outside by writing localStorage + mocking
 *  matchMedia so the provider picks up the expected resolved theme. */
function mountWithTheme(theme: 'light' | 'dark' | 'matrix' | 'sakura' | 'gotham') {
  localStorage.setItem('kronn:theme', theme);
  if (theme === 'matrix' || theme === 'sakura' || theme === 'gotham') {
    localStorage.setItem('kronn:unlockedThemes', JSON.stringify([theme]));
  }
  return render(
    <ThemeProvider>
      <ThemeEffects />
    </ThemeProvider>
  );
}

describe('ThemeEffects', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    document.documentElement.removeAttribute('data-theme');
    // Mock prefers-reduced-motion so the sakura handler always installs
    // (tests below verify matrix pulses, which don't depend on that).
    vi.spyOn(window, 'matchMedia').mockImplementation(
      (q: string) => ({
        matches: false,
        media: q,
        addEventListener: () => {},
        removeEventListener: () => {},
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        dispatchEvent: () => true,
      }) as unknown as MediaQueryList,
    );
  });
  afterEach(() => {
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('renders no overlay in light/dark themes', () => {
    const { container } = mountWithTheme('dark');
    expect(container.querySelector('.theme-effects-root')).toBeNull();
  });

  it('renders 6 sakura petals in the sakura theme', () => {
    const { container } = mountWithTheme('sakura');
    expect(container.querySelectorAll('.sakura-petal').length).toBe(6);
    expect(container.querySelectorAll('.sakura-petal-inner').length).toBe(6);
  });

  it('renders the bat-signal + 3 bats in the gotham theme', () => {
    const { container } = mountWithTheme('gotham');
    expect(container.querySelector('.bat-signal')).not.toBeNull();
    expect(container.querySelectorAll('.bat').length).toBe(3);
  });

  it('emits periodic matrix:pulse events in the matrix theme', () => {
    const listener = vi.fn();
    window.addEventListener('matrix:pulse', listener);
    mountWithTheme('matrix');

    // Pulses fire at random intervals of 8-14s. Over a 30s window we
    // expect at least 2 (worst case: 14s + 14s + a tail). Use
    // `>=` rather than exact equality because the random roll can
    // squeeze an extra pulse in.
    act(() => { vi.advanceTimersByTime(30000); });
    const firstWindowCalls = listener.mock.calls.length;
    expect(firstWindowCalls).toBeGreaterThanOrEqual(2);

    // And more pulses land if we keep advancing.
    act(() => { vi.advanceTimersByTime(30000); });
    expect(listener.mock.calls.length).toBeGreaterThan(firstWindowCalls);

    window.removeEventListener('matrix:pulse', listener);
  });

  it('does NOT emit matrix:pulse when the theme is not matrix', () => {
    const listener = vi.fn();
    window.addEventListener('matrix:pulse', listener);
    mountWithTheme('sakura');
    act(() => { vi.advanceTimersByTime(30000); });
    expect(listener).not.toHaveBeenCalled();
    window.removeEventListener('matrix:pulse', listener);
  });
});

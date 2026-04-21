import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ThemeProvider, useTheme } from '../ThemeContext';
import { themes as themesApi } from '../api';

vi.mock('../api', () => ({
  themes: { unlock: vi.fn() },
}));

function TestConsumer() {
  const { theme, setTheme, unlockedThemes, unlockTheme } = useTheme();
  return (
    <div>
      <span data-testid="theme">{theme}</span>
      <span data-testid="unlocked">{unlockedThemes.join(',')}</span>
      <button data-testid="set-light" onClick={() => setTheme('light')}>Light</button>
      <button data-testid="set-dark" onClick={() => setTheme('dark')}>Dark</button>
      <button data-testid="set-system" onClick={() => setTheme('system')}>System</button>
      <button data-testid="set-matrix" onClick={() => setTheme('matrix')}>Matrix</button>
      <button
        data-testid="unlock"
        onClick={async () => {
          try { await unlockTheme('any-code'); } catch { /* swallowed in test */ }
        }}
      >
        Unlock
      </button>
    </div>
  );
}

describe('ThemeContext', () => {
  beforeEach(() => {
    localStorage.clear();
    document.documentElement.removeAttribute('data-theme');
  });

  it('defaults to system mode', () => {
    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );
    expect(screen.getByTestId('theme')).toHaveTextContent('system');
  });

  it('switches to light theme', async () => {
    const user = userEvent.setup();
    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    await user.click(screen.getByTestId('set-light'));

    expect(screen.getByTestId('theme')).toHaveTextContent('light');
    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
  });

  it('switches to dark theme', async () => {
    const user = userEvent.setup();
    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    await user.click(screen.getByTestId('set-dark'));

    expect(screen.getByTestId('theme')).toHaveTextContent('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
  });

  it('persists theme to localStorage', async () => {
    const user = userEvent.setup();
    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    await user.click(screen.getByTestId('set-light'));

    expect(localStorage.getItem('kronn:theme')).toBe('light');
  });

  it('restores theme from localStorage', () => {
    localStorage.setItem('kronn:theme', 'dark');

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    expect(screen.getByTestId('theme')).toHaveTextContent('dark');
    expect(document.documentElement.getAttribute('data-theme')).toBe('dark');
  });

  it('ignores invalid localStorage value', () => {
    localStorage.setItem('kronn:theme', 'invalid');

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    expect(screen.getByTestId('theme')).toHaveTextContent('system');
  });

  it('applies data-theme attribute on mount', () => {
    localStorage.setItem('kronn:theme', 'light');

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    expect(document.documentElement.getAttribute('data-theme')).toBe('light');
  });

  // ─── Secret themes (matrix, sakura) ──────────────────────────────────

  it('ignores setTheme for a secret theme that is not unlocked', async () => {
    const user = userEvent.setup();
    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    await user.click(screen.getByTestId('set-matrix'));

    // setTheme must refuse to apply an unlocked-required theme when the
    // user hasn't unlocked it — stays on system, no data-theme flip.
    expect(screen.getByTestId('theme')).toHaveTextContent('system');
    expect(localStorage.getItem('kronn:theme')).toBeNull();
  });

  it('unlockTheme persists the unlocked theme and enables setTheme', async () => {
    const user = userEvent.setup();
    vi.mocked(themesApi.unlock).mockResolvedValueOnce({
      unlocks: [{ kind: 'theme', name: 'matrix' }],
    });

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    await user.click(screen.getByTestId('unlock'));
    await user.click(screen.getByTestId('set-matrix'));

    expect(screen.getByTestId('unlocked')).toHaveTextContent('matrix');
    expect(screen.getByTestId('theme')).toHaveTextContent('matrix');
    expect(document.documentElement.getAttribute('data-theme')).toBe('matrix');
    expect(JSON.parse(localStorage.getItem('kronn:unlockedThemes') ?? '[]')).toEqual(['matrix']);
  });

  it('handles a bundle unlock returning several themes + a profile', async () => {
    // Emulates the Batman code: backend returns profile + gotham theme
    // in one call. The context should persist the theme locally; the
    // profile entry is passed back to the caller untouched (handler
    // elsewhere refetches /api/profiles).
    const user = userEvent.setup();
    vi.mocked(themesApi.unlock).mockResolvedValueOnce({
      unlocks: [
        { kind: 'profile', name: 'batman' },
        { kind: 'theme', name: 'gotham' },
      ],
    });

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    await user.click(screen.getByTestId('unlock'));

    expect(screen.getByTestId('unlocked')).toHaveTextContent('gotham');
    expect(JSON.parse(localStorage.getItem('kronn:unlockedThemes') ?? '[]')).toEqual(['gotham']);
  });

  it('restores unlocked list from localStorage on mount', async () => {
    localStorage.setItem('kronn:unlockedThemes', JSON.stringify(['sakura']));
    const user = userEvent.setup();

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    expect(screen.getByTestId('unlocked')).toHaveTextContent('sakura');
    // setTheme for 'matrix' should still be rejected (only sakura was unlocked)
    await user.click(screen.getByTestId('set-matrix'));
    expect(screen.getByTestId('theme')).toHaveTextContent('system');
  });

  it('degrades a stored secret theme to system when no longer unlocked', () => {
    // User persisted 'matrix' but their unlocked list is empty (e.g.
    // wiped on a different browser). The provider must not apply it.
    localStorage.setItem('kronn:theme', 'matrix');
    localStorage.setItem('kronn:unlockedThemes', JSON.stringify([]));

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    expect(screen.getByTestId('theme')).toHaveTextContent('system');
  });

  it('rejects a payload with no known theme and no profile', async () => {
    const user = userEvent.setup();
    // Server returns an unknown-theme name and no profile → unlock must
    // throw, localStorage unchanged. (Bundle/server drift defense.)
    vi.mocked(themesApi.unlock).mockResolvedValueOnce({
      unlocks: [{ kind: 'theme', name: 'unknown-theme' }],
    });

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    await user.click(screen.getByTestId('unlock'));

    expect(screen.getByTestId('unlocked')).toHaveTextContent('');
    expect(localStorage.getItem('kronn:unlockedThemes')).toBeNull();
  });

  it('filters tampered unlocked-list entries on mount', () => {
    // A curious user edits localStorage directly. We must ignore values
    // that are not valid secret themes — they'll get no free upgrade.
    localStorage.setItem('kronn:unlockedThemes', JSON.stringify(['matrix', 'light', 'bogus']));

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    // Only 'matrix' survives (light is always-available, bogus is unknown)
    expect(screen.getByTestId('unlocked')).toHaveTextContent('matrix');
  });

  it('reacts to OS theme change in system mode', () => {
    let changeHandler: (() => void) | null = null;
    const mql = {
      matches: false,
      addEventListener: (_: string, handler: () => void) => { changeHandler = handler; },
      removeEventListener: vi.fn(),
    };
    vi.spyOn(window, 'matchMedia').mockReturnValue(mql as unknown as MediaQueryList);

    render(
      <ThemeProvider>
        <TestConsumer />
      </ThemeProvider>
    );

    // Simulate OS switching to light
    mql.matches = true;
    act(() => { changeHandler?.(); });

    expect(document.documentElement.getAttribute('data-theme')).toBe('light');

    vi.restoreAllMocks();
  });
});

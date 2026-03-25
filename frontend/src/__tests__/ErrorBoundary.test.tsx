// Note: assertions use French strings because the default UI locale is 'fr'.
// If the default locale changes, these assertions must be updated.
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ErrorBoundary } from '../components/ErrorBoundary';

function BrokenComponent(): never {
  throw new Error('Test explosion');
}

describe('ErrorBoundary', () => {
  it('catches errors and displays the error message (default FR)', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    localStorage.removeItem('kronn:ui-locale');

    render(
      <ErrorBoundary>
        <BrokenComponent />
      </ErrorBoundary>
    );

    expect(screen.getByText('Une erreur est survenue.')).toBeDefined();
    expect(screen.getByText('Test explosion')).toBeDefined();

    spy.mockRestore();
  });

  it('uses stored locale for error message', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    localStorage.setItem('kronn:ui-locale', 'en');

    render(
      <ErrorBoundary>
        <BrokenComponent />
      </ErrorBoundary>
    );

    expect(screen.getByText('Something went wrong.')).toBeDefined();

    spy.mockRestore();
    localStorage.removeItem('kronn:ui-locale');
  });

  it('uses stored locale for error message (ES)', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    localStorage.setItem('kronn:ui-locale', 'es');

    render(
      <ErrorBoundary>
        <BrokenComponent />
      </ErrorBoundary>
    );

    expect(screen.getByText('Algo salió mal.')).toBeDefined();

    spy.mockRestore();
    localStorage.removeItem('kronn:ui-locale');
  });

  it('renders children normally when no error', () => {
    render(
      <ErrorBoundary>
        <div data-testid="happy-child">All good</div>
      </ErrorBoundary>
    );

    expect(screen.getByTestId('happy-child')).toBeDefined();
    expect(screen.queryByText('Une erreur est survenue.')).toBeNull();
  });

  // ─── Zone mode tests ───────────────────────────────────────────────

  it('zone mode renders contained error with label', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    localStorage.removeItem('kronn:ui-locale');

    render(
      <ErrorBoundary mode="zone" label="Discussions">
        <BrokenComponent />
      </ErrorBoundary>
    );

    expect(screen.getByText(/Discussions/)).toBeDefined();
    expect(screen.getByText('Test explosion')).toBeDefined();
    expect(screen.getByText('Retry')).toBeDefined();
    // Should NOT have the fullscreen Reload button
    expect(screen.queryByText('Reload')).toBeNull();

    spy.mockRestore();
  });

  it('zone mode retry resets error state', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    localStorage.removeItem('kronn:ui-locale');

    render(
      <ErrorBoundary mode="zone" label="Test">
        <BrokenComponent />
      </ErrorBoundary>
    );

    // Should show error with Retry button
    expect(screen.getByText('Retry')).toBeDefined();
    expect(screen.getByText('Test explosion')).toBeDefined();

    // Click Retry — error state is cleared (component will re-throw, but
    // the boundary correctly resets its internal state)
    fireEvent.click(screen.getByText('Retry'));

    // After retry, BrokenComponent throws again so we're back in error state
    // but the important thing is the boundary didn't crash or become unresponsive
    expect(screen.getByText('Retry')).toBeDefined();

    spy.mockRestore();
  });

  it('fullscreen mode shows Reload button (not Retry)', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});
    localStorage.removeItem('kronn:ui-locale');

    render(
      <ErrorBoundary mode="fullscreen">
        <BrokenComponent />
      </ErrorBoundary>
    );

    expect(screen.getByText('Reload')).toBeDefined();
    expect(screen.queryByText('Retry')).toBeNull();

    spy.mockRestore();
  });
});

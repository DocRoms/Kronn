// Note: assertions use French strings because the default UI locale is 'fr'.
// If the default locale changes, these assertions must be updated.
import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
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
});

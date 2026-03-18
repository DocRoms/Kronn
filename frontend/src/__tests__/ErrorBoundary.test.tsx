import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { ErrorBoundary } from '../components/ErrorBoundary';

function BrokenComponent(): never {
  throw new Error('Test explosion');
}

describe('ErrorBoundary', () => {
  it('catches errors and displays the error message', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {});

    render(
      <ErrorBoundary>
        <BrokenComponent />
      </ErrorBoundary>
    );

    expect(screen.getByText('Something went wrong.')).toBeDefined();
    expect(screen.getByText('Test explosion')).toBeDefined();

    spy.mockRestore();
  });

  it('renders children normally when no error', () => {
    render(
      <ErrorBoundary>
        <div data-testid="happy-child">All good</div>
      </ErrorBoundary>
    );

    expect(screen.getByTestId('happy-child')).toBeDefined();
    expect(screen.queryByText('Something went wrong.')).toBeNull();
  });
});

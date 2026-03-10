import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { Component } from 'react';
import type { ReactNode, ErrorInfo } from 'react';

/** Minimal ErrorBoundary matching the one in App.tsx */
class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };
  static getDerivedStateFromError(error: Error) { return { error }; }
  componentDidCatch(_error: Error, _info: ErrorInfo) {}
  render() {
    if (this.state.error) {
      return <div data-testid="error-ui">{this.state.error.message}</div>;
    }
    return this.props.children;
  }
}

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

    expect(screen.getByTestId('error-ui')).toBeDefined();
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
    expect(screen.queryByTestId('error-ui')).toBeNull();
  });
});

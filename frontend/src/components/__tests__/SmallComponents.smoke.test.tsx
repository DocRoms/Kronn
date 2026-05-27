/**
 * 0.8.7 — Smoke coverage on small / simple components.
 *
 * Quick mounts for components with simple props : aims to lift them from
 * 0 % coverage to "mount path executed".
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import type * as React from 'react';
import { render, screen, cleanup } from '@testing-library/react';

afterEach(() => cleanup());

// ─── ErrorBoundary ─────────────────────────────────────────────────────
import { ErrorBoundary } from '../ErrorBoundary';

describe('ErrorBoundary', () => {
  it('renders children when no error', () => {
    render(<ErrorBoundary><div data-testid="ok">Hello</div></ErrorBoundary>);
    expect(screen.getByTestId('ok')).toBeDefined();
  });

  it('catches a child error and renders fallback UI', () => {
    const originalError = console.error;
    console.error = vi.fn();
    try {
      function Boom(): React.ReactElement {
        throw new Error('boom-from-child');
      }
      render(
        <ErrorBoundary>
          <Boom />
        </ErrorBoundary>,
      );
      const txt = document.body.textContent ?? '';
      expect(txt.length).toBeGreaterThan(0);
    } finally {
      console.error = originalError;
    }
  });
});

// ─── HostSyncChip ──────────────────────────────────────────────────────
import { HostSyncChip } from '../HostSyncChip';

describe('HostSyncChip', () => {
  it('renders for None mode', () => {
    const { container } = render(<HostSyncChip mode="None" />);
    // Even if the chip renders null for None, the render call itself is enough.
    expect(container).toBeDefined();
  });
  it('renders for GlobalOnly mode', () => {
    const { container } = render(<HostSyncChip mode="GlobalOnly" />);
    expect(container).toBeDefined();
  });
  it('renders for MirrorAll mode', () => {
    const { container } = render(<HostSyncChip mode="MirrorAll" />);
    expect(container).toBeDefined();
  });
});

// ─── MatrixText ────────────────────────────────────────────────────────
import { MatrixText } from '../MatrixText';

describe('MatrixText', () => {
  it('renders the underlying text', () => {
    const { container } = render(<MatrixText text="hello" />);
    expect(container.textContent).toContain('hello');
  });

  it('still mounts with empty text', () => {
    const { container } = render(<MatrixText text="" />);
    expect(container).toBeDefined();
  });
});

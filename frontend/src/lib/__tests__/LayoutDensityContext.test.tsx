import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent, cleanup } from '@testing-library/react';
import { LayoutDensityProvider, useLayoutDensity } from '../LayoutDensityContext';

function TestProbe() {
  const { density, setDensity } = useLayoutDensity();
  return (
    <div>
      <span data-testid="current">{density}</span>
      <button data-testid="set-small" onClick={() => setDensity('small')}>small</button>
      <button data-testid="set-medium" onClick={() => setDensity('medium')}>medium</button>
      <button data-testid="set-large" onClick={() => setDensity('large')}>large</button>
    </div>
  );
}

describe('LayoutDensityContext', () => {
  beforeEach(() => {
    try { localStorage.clear(); } catch { /* noop */ }
    document.documentElement.removeAttribute('data-density');
  });
  afterEach(() => cleanup());

  it('defaults to "medium" when nothing is stored (0.8.6 default change)', () => {
    render(
      <LayoutDensityProvider>
        <TestProbe />
      </LayoutDensityProvider>,
    );
    expect(screen.getByTestId('current').textContent).toBe('medium');
    expect(document.documentElement.getAttribute('data-density')).toBe('medium');
  });

  it('hydrates from localStorage on mount (user-chosen "small" persists across reloads)', () => {
    localStorage.setItem('kronn:layoutDensity', 'small');
    render(
      <LayoutDensityProvider>
        <TestProbe />
      </LayoutDensityProvider>,
    );
    expect(screen.getByTestId('current').textContent).toBe('small');
    expect(document.documentElement.getAttribute('data-density')).toBe('small');
  });

  it('falls back to medium default when localStorage has a garbage value', () => {
    localStorage.setItem('kronn:layoutDensity', 'gigantic');
    render(
      <LayoutDensityProvider>
        <TestProbe />
      </LayoutDensityProvider>,
    );
    expect(screen.getByTestId('current').textContent).toBe('medium');
  });

  it('setDensity persists to localStorage AND updates data-density', () => {
    render(
      <LayoutDensityProvider>
        <TestProbe />
      </LayoutDensityProvider>,
    );
    fireEvent.click(screen.getByTestId('set-large'));
    expect(screen.getByTestId('current').textContent).toBe('large');
    expect(localStorage.getItem('kronn:layoutDensity')).toBe('large');
    expect(document.documentElement.getAttribute('data-density')).toBe('large');
  });

  it('round-trips small → medium → large → small', () => {
    render(
      <LayoutDensityProvider>
        <TestProbe />
      </LayoutDensityProvider>,
    );
    fireEvent.click(screen.getByTestId('set-medium'));
    expect(document.documentElement.getAttribute('data-density')).toBe('medium');
    fireEvent.click(screen.getByTestId('set-large'));
    expect(document.documentElement.getAttribute('data-density')).toBe('large');
    fireEvent.click(screen.getByTestId('set-small'));
    expect(document.documentElement.getAttribute('data-density')).toBe('small');
    expect(localStorage.getItem('kronn:layoutDensity')).toBe('small');
  });
});

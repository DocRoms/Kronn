import { describe, it, expect, vi, afterEach } from 'vitest';
import { renderHook } from '@testing-library/react';

// Helper to mock window.matchMedia with a specific matches value
function mockMatchMedia(matches: boolean) {
  Object.defineProperty(window, 'matchMedia', {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches,
      media: query,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
    })),
  });
}

afterEach(() => {
  // Restore default (no match)
  mockMatchMedia(false);
});

describe('useIsMobile', () => {
  it('returns a boolean', async () => {
    mockMatchMedia(false);
    const { useIsMobile } = await import('../useMediaQuery');
    const { result } = renderHook(() => useIsMobile());
    expect(typeof result.current).toBe('boolean');
  });

  it('returns true when matchMedia matches (mobile viewport)', async () => {
    mockMatchMedia(true);
    const { useIsMobile } = await import('../useMediaQuery');
    const { result } = renderHook(() => useIsMobile());
    expect(result.current).toBe(true);
  });

  it('returns false when matchMedia does not match (desktop viewport)', async () => {
    mockMatchMedia(false);
    const { useIsMobile } = await import('../useMediaQuery');
    const { result } = renderHook(() => useIsMobile());
    expect(result.current).toBe(false);
  });

  it('uses default breakpoint of 768px', async () => {
    mockMatchMedia(false);
    const { useIsMobile } = await import('../useMediaQuery');
    renderHook(() => useIsMobile());
    expect(window.matchMedia).toHaveBeenCalledWith('(max-width: 767px)');
  });

  it('accepts a custom breakpoint', async () => {
    mockMatchMedia(false);
    const { useIsMobile } = await import('../useMediaQuery');
    renderHook(() => useIsMobile(1024));
    expect(window.matchMedia).toHaveBeenCalledWith('(max-width: 1023px)');
  });

  it('cleans up event listener on unmount', async () => {
    const removeListener = vi.fn();
    Object.defineProperty(window, 'matchMedia', {
      writable: true,
      value: vi.fn().mockImplementation((query: string) => ({
        matches: false,
        media: query,
        addEventListener: vi.fn(),
        removeEventListener: removeListener,
      })),
    });
    const { useIsMobile } = await import('../useMediaQuery');
    const { unmount } = renderHook(() => useIsMobile());
    unmount();
    expect(removeListener).toHaveBeenCalledWith('change', expect.any(Function));
  });
});

// KT-1883 — the app's top bar publishes its height, so the toast stack sits
// just under it below --kr-bp-md (toastStack.css).
import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, render } from '@testing-library/react';
import { useRef } from 'react';
import { APP_BAR_HEIGHT_VAR, useAppBarHeight } from '../useAppBarHeight';

function Bar() {
  const ref = useRef<HTMLElement>(null);
  useAppBarHeight(ref);
  return <nav ref={ref}>bar</nav>;
}

const root = document.documentElement;

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('useAppBarHeight', () => {
  it('publishes the bar height, follows its resizes and forgets it on unmount', () => {
    let height = 57.4;
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(() => ({ height } as DOMRect));
    let resize = () => {};
    vi.stubGlobal('ResizeObserver', class {
      constructor(cb: ResizeObserverCallback) { resize = () => cb([], this as unknown as ResizeObserver); }
      observe() {}
      disconnect() {}
    });
    const { unmount } = render(<Bar />);
    expect(root.style.getPropertyValue(APP_BAR_HEIGHT_VAR)).toBe('57px');

    height = 96;
    act(() => resize());
    expect(root.style.getPropertyValue(APP_BAR_HEIGHT_VAR)).toBe('96px');

    unmount();
    expect(root.style.getPropertyValue(APP_BAR_HEIGHT_VAR)).toBe('');
  });

  it('is the property the toast stack reads', async () => {
    const { readFileSync } = await import('node:fs');
    const { join, dirname } = await import('node:path');
    const { fileURLToPath } = await import('node:url');
    const css = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', 'toastStack.css'), 'utf8');
    expect(css).toContain(`var(${APP_BAR_HEIGHT_VAR},`);
  });
});

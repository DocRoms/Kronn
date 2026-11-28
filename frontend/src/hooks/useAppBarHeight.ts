import { useLayoutEffect, type RefObject } from 'react';

/** The custom property toastStack.css reads to sit under the app's top bar. */
export const APP_BAR_HEIGHT_VAR = '--app-bar-h';

/** KT-1883 — publish the height of the app's top bar on the document, so the
 *  toast stack can sit just under it below --kr-bp-md. Dashboard publishes its
 *  nav, the standalone discussion mosaic its own toolbar. The bar's height is
 *  not a token: it follows the text size and the controls on a coarse
 *  pointer, so it is measured. */
export function useAppBarHeight(bar: RefObject<HTMLElement | null>): void {
  useLayoutEffect(() => {
    const el = bar.current;
    if (!el) return;
    const root = document.documentElement;
    // Measured, so not a --kr-* token; written literally so the theme audit
    // sees where it is declared.
    const publish = () => root.style.setProperty('--app-bar-h', `${Math.round(el.getBoundingClientRect().height)}px`);
    publish();
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(publish);
    observer?.observe(el);
    return () => {
      observer?.disconnect();
      root.style.removeProperty(APP_BAR_HEIGHT_VAR);
    };
  }, [bar]);
}

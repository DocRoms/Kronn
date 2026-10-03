import { createElement, lazy, type ComponentType } from 'react';

export type PreloadablePage<P extends object> = ComponentType<P> & { preload: () => Promise<unknown> };

/**
 * `React.lazy` that can be fetched ahead of use. Once preloaded, the page
 * renders directly: a lazy component suspends on its first render even when
 * its module is already in memory, which would flash the loading state on
 * every first visit to a tab.
 */
export function lazyPage<P extends object>(load: () => Promise<ComponentType<P>>): PreloadablePage<P> {
  let loaded: ComponentType<P> | null = null;
  let pending: Promise<ComponentType<P>> | null = null;
  const fetchPage = () => {
    pending ??= load().then(component => {
      loaded = component;
      return component;
    }).catch(error => {
      // A failed chunk must be retryable on the next visit.
      pending = null;
      throw error;
    });
    return pending;
  };
  const Lazy = lazy(() => fetchPage().then(component => ({ default: component })));
  const Page = ((props: P) => createElement(loaded ?? Lazy, props)) as PreloadablePage<P>;
  Page.preload = fetchPage;
  return Page;
}

/** Fetch every page in the background once the browser is idle. */
export function preloadPagesWhenIdle(pages: { preload: () => Promise<unknown> }[]): () => void {
  const run = () => {
    for (const page of pages) void page.preload().catch(() => { /* retried on visit */ });
  };
  if (typeof window.requestIdleCallback === 'function') {
    const id = window.requestIdleCallback(run, { timeout: 3000 });
    return () => window.cancelIdleCallback(id);
  }
  const id = window.setTimeout(run, 1000);
  return () => window.clearTimeout(id);
}

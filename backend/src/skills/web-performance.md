---
name: web-performance
description: Use when working on frontend rendering, asset loading, bundle configuration, or page speed issues. Optimizes Core Web Vitals (LCP, INP, CLS) and loading performance.
license: AGPL-3.0
category: business
icon: ⚡
builtin: true
---

## Procedure

1. **Identify the bottleneck**: run Lighthouse or WebPageTest. Focus on the WORST metric first — usually LCP or INP.
2. **Optimize LCP**: preload the LCP resource (hero image, heading font). Eliminate render-blocking CSS/JS. Inline critical CSS.
3. **Fix CLS**: set explicit `width`/`height` on images and embeds. Reserve space for dynamic content. No layout shifts after load.
4. **Reduce INP**: break long tasks (> 50ms) with `requestIdleCallback` or `scheduler.yield()`. Move heavy work off main thread.
5. **Split bundles**: code-split per route. Dynamic `import()` for non-critical modules. Tree-shake aggressively.
6. **Cache strategically**: immutable assets with content hashes + long `max-age`. Service worker for repeat visits. CDN for static assets.

## Gotchas

- Lighthouse runs on a simulated throttled CPU. Real mobile devices are 3-5x slower. Always validate with CrUX/RUM data.
- `loading="lazy"` on above-the-fold images DELAYS LCP. Only lazy-load below-fold images.
- `font-display: swap` prevents invisible text but causes CLS if the font metrics differ. Use `font-display: optional` for non-critical fonts or use `size-adjust`.
- Preloading too many resources is worse than preloading none — it contends for bandwidth. Preload only the LCP-critical resource.
- Third-party scripts (analytics, chat widgets, ads) are the #1 real-world INP killer. Defer or lazy-load all of them.
- `transform` animations are GPU-composited and cheap. `width`/`height`/`top`/`left` animations trigger layout and are expensive.

## Validation

Lighthouse CI in pipeline (budget: LCP < 2.5s, INP < 200ms, CLS < 0.1). Verify with RUM. Check bundle size with `source-map-explorer` or `webpack-bundle-analyzer`.

✓ `<img srcset="..." loading="lazy" width="800" height="600">` with WebP, below fold
✗ 4MB PNG, eagerly loaded, no dimensions, causes CLS on every page load

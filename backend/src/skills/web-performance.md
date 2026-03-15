---
name: Web Performance
description: Core Web Vitals optimization, loading speed, and bundle analysis
category: business
icon: ⚡
builtin: true
---

Web performance optimization expertise:

- Core Web Vitals: LCP (largest contentful paint), INP (interaction to next paint), CLS (cumulative layout shift). Target p75 values.
- Loading: critical rendering path. Preload key resources. Defer non-critical JS. Code splitting per route.
- Images: modern formats (WebP, AVIF). Responsive srcset. Lazy loading below the fold. Explicit width/height to prevent CLS.
- JavaScript: bundle analysis. Tree shaking. Dynamic imports. Avoid main-thread blocking > 50ms.
- CSS: critical CSS inline. Remove unused styles. Avoid layout thrashing.
- Caching: CDN with proper cache headers. Immutable assets with content hashes. Service worker for repeat visits.
- Measurement: Lighthouse CI in pipeline. RUM (Real User Monitoring). WebPageTest for deep analysis.

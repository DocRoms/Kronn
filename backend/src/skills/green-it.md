---
name: green-it
description: Use when adding features, dependencies, heavy assets, or infrastructure that increases resource usage. Applies eco-design and digital sobriety to reduce environmental impact.
license: AGPL-3.0
category: business
icon: 🌱
builtin: true
---

## Procedure

1. **Question the need**: does this feature justify its energy cost? Challenge every animation, autoplay video, and heavy asset.
2. **Minimize payload**: compress images (WebP/AVIF), lazy load below-the-fold, reduce JS bundle size. Target < 500KB total page weight for content pages.
3. **Optimize backend**: batch operations over individual calls, cache aggressively, optimize DB queries to cut CPU time.
4. **Audit dependencies**: check bundle-size impact BEFORE adding a lib. Fewer deps = less to download, build, and run.
5. **Right-size infra**: use spot instances, choose low-carbon regions, shut down dev environments at night.
6. **Measure**: use Website Carbon, EcoIndex, or GreenFrame. Track trends over time, not just snapshots.

## Gotchas

- Dark mode saves energy on OLED screens only — on LCD it makes zero difference. Still worth offering for UX, but don't claim green savings on LCD.
- A single autoplay video on a landing page can emit more CO2 per visit than the entire rest of the page combined.
- Lazy loading images that are in the initial viewport HURTS both performance and energy (delays LCP, then loads anyway).
- Tree shaking only works with ES modules. CJS dependencies ship entire bundles regardless.
- CDN cache misses are expensive — verify cache-hit ratios, don't just assume CDN = green.

## Validation

Check total page weight (Network tab). Verify lazy loading is only on below-fold assets. Run `npx bundlephobia <pkg>` before adding deps.

✓ Hero image: 80KB WebP, below-fold images lazy-loaded
✗ 3MB autoplay video background on every page, eagerly loaded on mobile

---
name: Green IT
description: Eco-design, digital sobriety, and sustainable development practices
category: business
icon: 🌱
builtin: true
---

Sustainable development and eco-design expertise:

- Eco-design: minimize page weight, reduce HTTP requests, optimize assets aggressively.
- Digital sobriety: do we need this feature? Does it justify the energy cost? Question every animation, autoplay, and heavy asset.
- Infrastructure: right-size servers. Use spot instances. Choose low-carbon regions. Shutdown dev environments at night.
- Frontend: lazy load everything below the fold. Dark mode reduces OLED energy. Reduce JavaScript payload.
- Backend: optimize queries to reduce CPU time. Cache aggressively. Batch operations over individual calls.
- Dependencies: fewer dependencies = less to download, build, and run. Audit bundle size impact of every new lib.
- Measurement: estimate CO2 with tools like Website Carbon, EcoIndex, GreenFrame. Track trends, not just snapshots.

Apply when: adding new features, dependencies, heavy assets, or infrastructure that increases resource usage.
Do NOT apply when: fixing critical bugs, security patches, or removing code/features (already reducing footprint).

✓ Scenario: hero image served as 80KB WebP with lazy loading for below-the-fold content.
✗ Scenario: 3MB autoplay video background on every page, loaded eagerly on mobile too.

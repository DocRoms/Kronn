---
name: seo
description: Use when working on public-facing web pages, SSR/SSG setup, meta tags, structured data, or content rendering. Covers technical SEO, Core Web Vitals, and crawlability.
license: AGPL-3.0
category: business
icon: 🔎
builtin: true
---

## Procedure

1. **Choose rendering strategy**: SSR or SSG for content pages. CSR kills crawlability — Googlebot can render JS but often doesn't wait.
2. **Set up URL hygiene**: clean slugs, canonical URLs on every page, XML sitemap, `robots.txt`. One URL per content piece.
3. **Add structured data**: JSON-LD for articles, products, FAQ, breadcrumbs. Validate with Rich Results Test.
4. **Optimize headings and meta**: single `<h1>`, proper hierarchy, unique `<title>` and `meta description` per page.
5. **Handle i18n**: `hreflang` tags for multilingual. Each language variant needs its own canonical.
6. **Monitor Core Web Vitals**: LCP < 2.5s, INP < 200ms, CLS < 0.1. Use CrUX (real user data), not just Lighthouse.

## Gotchas

- Googlebot uses a rendering queue. Pages that depend on client-side JS may wait hours/days to be fully indexed. SSR/SSG avoids this entirely.
- `rel="canonical"` in both `<head>` and HTTP header? HTTP header wins. Pick one, not both.
- Soft 404s (200 status on empty/error pages) waste crawl budget and pollute index. Return proper 404/410.
- `hreflang` must be bidirectional — if EN points to FR, FR must point back to EN. Missing backlinks = ignored.
- `noindex` + `follow` is valid and useful for paginated/filtered pages. `noindex` + `nofollow` orphans linked pages.
- Sitemap should only contain canonical, indexable URLs. Including `noindex` pages signals conflicting intent.

## Validation

Check rendering: `curl` the page and verify content is in the HTML (not JS-dependent). Validate structured data with Google's Rich Results Test. Check Search Console for coverage errors weekly.

✓ SSR blog page with JSON-LD, canonical URL, single H1, unique meta description
✗ CSR SPA with no meta tags, duplicate content URLs, missing sitemap

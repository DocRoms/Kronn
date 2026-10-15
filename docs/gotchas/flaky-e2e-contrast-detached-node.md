# Flaky e2e contrast scan: a detached node comes back as `:root`

`a11y-dark-themes.spec.ts` re-measures the text nodes axe leaves undecided
(`bgGradient`, `pseudoContent`, `bgOverlap`) by querying their axe selector
again. Seen on CI in October 2026 (PR #245, `gotham: Settings`):

```
✗ :root (re-measured) — rgb(0, 0, 0) on rgb(20, 8, 34) = 1.09 (needs 4.5)
```

with no CSS or theme change in the diff, and no reproduction locally.

- **Why `:root`.** axe-core (4.13.0) computes a node's selector lazily, when
  the result is serialised (`DqElement.get selector`), not when the check
  ran. `generateSelector` queries the document for the candidate selector;
  a node React replaced in between is detached, matches nothing, and the
  fallback is the literal `:root`.
- **Why it fails.** The re-measure then reads `<html>` itself: its color is
  the UA default black, its background is the `#140822` painted by
  `index.html` for the boot screen, so the ratio is 1.09 whatever the theme.
- **Fix.** The helper counts such a target as `detached` and skips it; the
  log line shows `detached N` when it happened.
  [src: file: frontend/e2e/specs/a11y-dark-themes.spec.ts:146]
- **Not the same thing.** On a laptop the Settings scan also fails on
  `.api-call-logs-status-error` pills (4.47:1) when the local database holds
  API-call errors in the Debug section; CI has no such rows. That is real
  data-dependent noise, not this race.

Related: [flaky-rust-test-races](flaky-rust-test-races.md).

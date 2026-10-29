# e2e: axe reports `:root` for an element it could not address

**Symptom.** `a11y-dark-themes.spec.ts › <theme>: Settings` fails on one
re-measured node, always the same, on any dark theme:

```
✗ :root (re-measured) — rgb(0, 0, 0) on rgb(20, 8, 34) = 1.09 (needs 4.5)
```

Nothing on the page is black on purple. The failure comes and goes between two
runs of the same commit (PR #251, runs 38744668297 and 38747093853), and hit
four other branches the same day.

**Cause.** axe-core builds a unique CSS selector for every node it reports;
when it cannot (the element was re-rendered or detached between the analysis
and the report, so nothing on the page matches any more), it falls back to the
literal `:root` [src: url: https://github.com/dequelabs/axe-core/blob/v4.13.0/lib/core/utils/get-selector.js#L402].
The spec re-measures axe's undecided nodes by selector
[src: file: frontend/e2e/specs/a11y-dark-themes.spec.ts:128]; `document.querySelector(':root')`
is `<html>`, whose own `color` is the UA default black, measured against the
page background: a violation on text nobody sees. Settings leaves ~490 nodes
undecided for pseudo-content, so a single transient node is likely there.

**Fix.** The spec drops bare `:root` targets before re-measuring and counts
them as `detached` in its `[contrast]` log line, and fails if more than three
are lost, so a page that keeps re-rendering cannot empty the scan. Selectors of
the form `:root > …` come from the same fallback with a partial path
[src: url: https://github.com/dequelabs/axe-core/blob/v4.13.0/lib/core/utils/get-selector.js#L400];
they are kept, and `remeasure` skips one that matches nothing.

**Related.** [ci-macos-portability-memory](ci-macos-portability-memory.md)
for the other CI time sink of the same investigation.

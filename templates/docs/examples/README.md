# Worked examples

A worked example shows a real, complete instance of a recurring task in this
project — a full request handler, a full component + its test, a full
migration — so an agent copies a working pattern instead of re-deriving one
from scattered snippets.

## Conventions

- One file per pattern. Name them by what they demonstrate:
  - `new-api-endpoint.md` — a full handler + route registration + test.
  - `new-frontend-component.md` — a full component + its test.
- Every example must cite its source with `[src: file: <path>:<line>]` — a
  worked example that isn't traceable to real code is a fabrication risk, not
  a pattern.
- Keep it short: the finished shape + a 2-3 sentence note on what to adapt.
  Not a tutorial.

## Template

See [TEMPLATE.md](TEMPLATE.md) for the per-file shape. Copy + rename when
adding a new example.

## Updating

An example goes stale the same way a sequence diagram does (see
[`architecture/sequences/README.md`](../architecture/sequences/README.md)):
when the pattern it shows changes, update the example in the same PR.

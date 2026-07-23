# TD-20260509 — React 19/20 strict effect rules

## Context

When we lifted `react-hooks` to the strict React-19/20 ruleset to prepare for
React 20, several lints triggered on patterns that work today but won't match
future React expectations. Demoted from `error` to `warn` in
`frontend/eslint.config.js` so CI stays green:

- `react-hooks/purity`
- `react-hooks/immutability`
- `react-hooks/refs`
- `react-hooks/set-state-in-effect`
- `react-hooks/preserve-manual-memoization`
- `react-hooks/exhaustive-deps` (mostly "adding this dep loops the effect")

Ad-hoc TS-strict warnings ride alongside:
- `@typescript-eslint/no-non-null-assertion`
- `@typescript-eslint/no-explicit-any`

## Current count (2026-07-24)

`pnpm exec eslint src/ -f json` reports **161 warnings, 0 errors**:

| Rule | Count | Category |
|---|---|---|
| `react-hooks/set-state-in-effect` | 58 | Heavy refactor each (often state derived from props) |
| `react-hooks/immutability` | 27 | Mutation in render / effect |
| `react-refresh/only-export-components` | 22 | Split shared exports from component modules |
| `react-hooks/exhaustive-deps` | 18 | Mostly stable refs / useCallback opportunities |
| `react-hooks/refs` | 13 | Refs for non-DOM values |
| `@typescript-eslint/no-non-null-assertion` | 12 | Add explicit narrowing |
| `react-hooks/purity` | 4 | Pure-render rule violations |
| `no-restricted-syntax` | 3 | Project-specific restricted patterns |
| `react-hooks/preserve-manual-memoization` | 2 | Memo broken by inner ref change |
| `no-console` | 2 | Debug output |

The earlier 2026-05-10 snapshot was 99 warnings. The rule set and code surface
have evolved since then, so the old count must not be used as the CI baseline.
The inventory above was measured directly after the 0.8.13 quick-win pass.

## Patterns applied for the cleared cases

- `Map.get(k)!.x` → `let g = m.get(k); if (!g) { g = …; m.set(k, g); } g.x` (extract local).
- TS narrowing lost across IIFE / async closure → hoist to `const` before the closure.
- `arr.find(...)!` → explicit `if (!found) return null;` guard.
- `[a, b]` array → `as Array<[string, string]>` cast for tuple destructure.
- Derived state setter inside an effect with stale ref → drop the ref and recompute via `useMemo`.

## Why the remaining warnings stay

Each warning needs per-file analysis:
- `set-state-in-effect`: usually means "state is derived from props" — should become `useMemo`. Easy 1-2-line fixes once you understand the data flow, but you have to read each effect.
- `exhaustive-deps`: half are "ref doesn't need to be a dep", half are "yes, this dep is missing — but adding it loops the effect" (need `useCallback` or split).
- `immutability` / `refs` / `purity`: often deliberate React-18 patterns we'll need to refactor when React 20 actually ships.

No automated fix for these — `eslint --fix` doesn't help.

## Plan

### Why per-file passes, not bulk

Each warning needs contextual analysis:

- **`set-state-in-effect`** (58 hits) — usually means "reset state on
  prop change" or "derive state from props with a side-effect".
  Three legitimate fixes per case:
  - Replace with `useMemo` (when no real side effect)
  - Replace with `key`-based component remount (when reset is the
    point)
  - Use `useEffectEvent` (not yet stable in React 19) or accept and
    document the pattern (when external side-effect is unavoidable
    e.g. `stopTts()` on disc switch).
- **`exhaustive-deps`** (18 hits) — half are stable callbacks that
  could move into `useCallback` (mechanical fix), half are
  derived-state-with-loop traps (need split effects).
- **`refs`** (13 hits) — refs storing non-DOM values. Sometimes
  safe to migrate to `useState` + `useEffect` pair, sometimes not
  (the ref's stability is load-bearing for race guards — see
  `feedback_race_guards.md`).
- **`immutability`** (27 hits) — mutation in render. Almost always
  fixable by hoisting the value into a `useMemo`.

Recompute per-file hotspots from the JSON lint output before each pass; the
2026-05 file ranking is no longer reliable.

**DOD per pass**: zero warnings of the targeted category in the targeted file, build & tests stay green.

## Tests to not break

- `pnpm test` (2619 passing on the 2026-07-24 snapshot)
- `pnpm test:e2e` (smoke + tour + 36 specs)
- No regression on auto-scroll, guided tour, or SSE streaming — that's where most of the delicate `useEffect`s live.

## Pointers

- ESLint config: `frontend/eslint.config.js:24-37`
- Memory: `feedback_race_guards.md` (about `useRef` for async)
- Per-rule list of warning sites: `pnpm exec eslint src/ -f stylish` (default) or wrap in the helper script under `frontend/scripts/lint-react19.sh` if we add one

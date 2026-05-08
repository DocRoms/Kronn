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

## Current count (2026-05-10, after polish session)

`pnpm exec eslint src/` reports **99 warnings, 0 errors**:

| Rule | Count | Category |
|---|---|---|
| `react-hooks/set-state-in-effect` | 42 | Heavy refactor each (often state derived from props) |
| `react-hooks/exhaustive-deps` | 20 | Mostly stable refs / useCallback opportunities |
| `react-hooks/immutability` | 14 | Mutation in render / effect |
| `react-hooks/refs` | 10 | Refs for non-DOM values |
| `react-hooks/purity` | 4 | Pure-render rule violations |
| `@typescript-eslint/no-explicit-any` | 4 | All in SSE event dispatchers (`api.ts:fetchAndParseSSE` callsites) |
| `react-hooks/preserve-manual-memoization` | 2 | Memo broken by inner ref change |

**Cleared this session** (122 → 99, −23):
- `@typescript-eslint/no-non-null-assertion`: **21 → 0** ✅
- `@typescript-eslint/no-explicit-any`: 9 → 4 (5 cleared in MessageBubble / DiscussionSidebar / DiscussionsPage; 4 SSE ones left)

## Patterns applied for the cleared cases

- `Map.get(k)!.x` → `let g = m.get(k); if (!g) { g = …; m.set(k, g); } g.x` (extract local).
- TS narrowing lost across IIFE / async closure → hoist to `const` before the closure.
- `arr.find(...)!` → explicit `if (!found) return null;` guard.
- `[a, b]` array → `as Array<[string, string]>` cast for tuple destructure.
- Derived state setter inside an effect with stale ref → drop the ref and recompute via `useMemo`.

## Why the remaining warnings stay

Each of the 99 needs per-file analysis:
- `set-state-in-effect`: usually means "state is derived from props" — should become `useMemo`. Easy 1-2-line fixes once you understand the data flow, but you have to read each effect.
- `exhaustive-deps`: half are "ref doesn't need to be a dep", half are "yes, this dep is missing — but adding it loops the effect" (need `useCallback` or split).
- `immutability` / `refs` / `purity`: often deliberate React-18 patterns we'll need to refactor when React 20 actually ships.

No automated fix for these — `eslint --fix` doesn't help.

## Plan

### Why per-file passes, not bulk

Each warning needs contextual analysis:

- **`set-state-in-effect`** (42 hits) — usually means "reset state on
  prop change" or "derive state from props with a side-effect".
  Three legitimate fixes per case:
  - Replace with `useMemo` (when no real side effect)
  - Replace with `key`-based component remount (when reset is the
    point)
  - Use `useEffectEvent` (not yet stable in React 19) or accept and
    document the pattern (when external side-effect is unavoidable
    e.g. `stopTts()` on disc switch).
- **`exhaustive-deps`** (20 hits) — half are stable callbacks that
  could move into `useCallback` (mechanical fix), half are
  derived-state-with-loop traps (need split effects).
- **`refs`** (10 hits) — refs storing non-DOM values. Sometimes
  safe to migrate to `useState` + `useEffect` pair, sometimes not
  (the ref's stability is load-bearing for race guards — see
  `feedback_race_guards.md`).
- **`immutability`** (14 hits) — mutation in render. Almost always
  fixable by hoisting the value into a `useMemo`.

By directory, in priority order (warning hotspots):

1. **`pages/DiscussionsPage.tsx`** (~31 hits, post-cleanup) — biggest single file.
2. **`components/workflows/WorkflowDetail.tsx`** (~10 hits) — ApiCallStepCard & co.
3. **`pages/Dashboard.tsx`** (~7 hits)
4. **`components/ChatInput.tsx`** (~6 hits)
5. **`components/workflows/{ApiCallStepCard,WorkflowWizard}.tsx`** (~5 each)
6. The long tail (everything else, ≤4 hits per file).

**DOD per pass**: zero warnings of the targeted category in the targeted file, build & tests stay green.

## Tests to not break

- `pnpm test --run` (currently 1082 passing)
- `pnpm test:e2e` (smoke + tour + 36 specs)
- No regression on auto-scroll, guided tour, or SSE streaming — that's where most of the delicate `useEffect`s live.

## Pointers

- ESLint config: `frontend/eslint.config.js:24-37`
- Memory: `feedback_race_guards.md` (about `useRef` for async)
- Per-rule list of warning sites: `pnpm exec eslint src/ -f stylish` (default) or wrap in the helper script under `frontend/scripts/lint-react19.sh` if we add one

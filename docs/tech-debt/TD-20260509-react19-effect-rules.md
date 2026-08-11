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

## Current count (2026-08-11 — fourth contextual pass)

The first maintenance pass remeasured **164 warnings, 0 errors**. After four
contextual batches, `pnpm exec eslint src/ -f json` reports **72 warnings,
0 errors**:

| Rule | Count | Category |
|---|---|---|
| `react-hooks/set-state-in-effect` | 43 | Heavy refactor each (often state derived from props) |
| `react-hooks/immutability` | 19 | Mutation in render / effect |
| `react-hooks/purity` | 4 | Pure-render rule violations |
| `no-restricted-syntax` | 3 | Project-specific restricted patterns |
| `react-hooks/preserve-manual-memoization` | 2 | Memo broken by inner ref change |
| `react-hooks/refs` | 1 | Ref observed during render |

The first batch brought ten targeted component files to zero warnings without
disabling a rule: `SourceCodeViewer`, `AiDocViewer`, `AgentQuestionForm`,
`AuditRecapPanel`, `DiscParticipantsHeader`, `QPHistoryDrawer`,
`DiscussionPlanPanel`, `DiscussionSessionBinding`, `Dropdown` and
`MermaidDiagram`. The earlier snapshots remain historical context, not the CI
baseline.

The second batch removes another 33 warnings across boot/setup, Settings,
Custom API and usage helpers, theme/user-context components, responsive state,
WebSocket/API hooks and callback-ref hooks. Pure helpers and contexts now live
outside component-only Fast Refresh modules; external subscriptions use
`useSyncExternalStore` or effect-managed callbacks; latest-callback refs update
during the layout phase, before browser callbacks can observe a stale handler;
and initial async reads use guarded promise continuations instead of scheduler
indirection.

The third batch clears the remaining Fast Refresh and non-null assertion
categories. Message parsing, discussion unread labels, workflow UI helpers and
API-assistant transforms now live in pure modules, while the toast item is a
component-only boundary. Tests import the same production helpers directly,
so extracting them does not create parallel test-only implementations.

The fourth batch removes all twelve `exhaustive-deps` warnings rather than
silencing them. Discussion message/language/workspace values are explicit
dependencies, the WebSocket callback now depends on stable callbacks declared
before it, empty-list fallbacks are referentially stable, and latest-handler
refs update in the layout phase. It also removes obsolete render-time ref
access from the mention picker and API-call filters. Oxlint is consequently a
zero-warning gate; ESLint's 72-warning residual is pinned in CI as the new
ratchet.

## Patterns applied for the cleared cases

- `Map.get(k)!.x` → `let g = m.get(k); if (!g) { g = …; m.set(k, g); } g.x` (extract local).
- TS narrowing lost across IIFE / async closure → hoist to `const` before the closure.
- `arr.find(...)!` → explicit `if (!found) return null;` guard.
- `[a, b]` array → `as Array<[string, string]>` cast for tuple destructure.
- Derived state setter inside an effect with stale ref → drop the ref and recompute via `useMemo`.
- Async data reset in an effect → store a result keyed by project/path/query and
  derive loading/empty state from whether the result matches the current key.
- Concurrent async searches → invalidate the previous effect generation before
  applying a response, so a slower earlier request cannot replace the current
  result or select a stale file.
- Async retry actions → reset loading and error state in the user event, then
  reuse the same generation-guarded reader as the initial load.
- Latest-callback refs used by browser subscriptions → update the ref in a
  layout effect so events cannot observe a commit-to-passive-effect stale
  window.
- Local form state that must reset on identity change → key a small inner
  component by that identity instead of issuing a reset render from an effect.
- Picker highlight derived from an external value → initialize it in the open
  event that needs it, rather than continuously mirroring props into state.
- Pure helpers exported from component modules → move them to a dedicated
  library module so Fast Refresh keeps a component-only boundary.

## Why the remaining warnings stay

Each warning needs per-file analysis:
- `set-state-in-effect`: usually means "state is derived from props" — should become `useMemo`. Easy 1-2-line fixes once you understand the data flow, but you have to read each effect.
- `immutability` / `refs` / `purity`: often deliberate React-18 patterns we'll need to refactor when React 20 actually ships.

The Vitest suite also emits historical `act(...)` and aborted-mock-fetch noise.
Those messages do not affect its result, but they obscure real failures and
must be removed contextually from the owning test suites rather than suppressed
globally.

No automated fix for these — `eslint --fix` doesn't help.

## Plan

### Why per-file passes, not bulk

Each warning needs contextual analysis:

- **`set-state-in-effect`** (43 hits) — usually means "reset state on
  prop change" or "derive state from props with a side-effect".
  Three legitimate fixes per case:
  - Replace with `useMemo` (when no real side effect)
  - Replace with `key`-based component remount (when reset is the
    point)
  - Use `useEffectEvent` (not yet stable in React 19) or accept and
    document the pattern (when external side-effect is unavoidable
    e.g. `stopTts()` on disc switch).
- **`refs`** (1 hit) — a ref is observed during render. Sometimes
  safe to migrate to `useState` + `useEffect` pair, sometimes not
  (the ref's stability is load-bearing for race guards — see
  `feedback_race_guards.md`).
- **`immutability`** (19 hits) — mutation in render. Almost always
  fixable by hoisting the value into a `useMemo`.

Recompute per-file hotspots from the JSON lint output before each pass; the
2026-05 file ranking is no longer reliable.

**DOD per pass**: zero warnings of the targeted category in the targeted file, build & tests stay green.

## Tests to not break

- `pnpm test`
- `pnpm test:e2e`
- No regression on auto-scroll, guided tour, or SSE streaming — that's where most of the delicate `useEffect`s live.

## Pointers

- ESLint config: `frontend/eslint.config.js:24-37`
- Memory: `feedback_race_guards.md` (about `useRef` for async)
- Per-rule list of warning sites: `pnpm exec eslint src/ -f stylish` (default) or wrap in the helper script under `frontend/scripts/lint-react19.sh` if we add one

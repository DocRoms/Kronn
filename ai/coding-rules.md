# Coding rules (AI contract)

> Glossary: `ai/glossary.md`.

## Global

- Prefer smallest diffs. Avoid drive-by refactors.
- Follow existing naming in adjacent code. Avoid generic names (`Helper`, `Utils`).
- No CSS framework — all styles are inline React `style={{}}` objects.
- No emojis in code unless user explicitly requests them.

## Rust (backend)

- **Framework**: axum 0.7 with tokio async runtime.
- **Error handling**: `anyhow::Result` for internal, `ApiResponse` wrapper for HTTP.
- **Serialization**: serde with `#[serde(rename_all = "snake_case")]` on enums.
- **Route registration**: chain methods on same path — `.route("/path", get(h).post(h2))`, never two `.route()` calls with same path (axum panics).
- **Derive requirements**: add `PartialEq` to any enum used in `==` / `!=` / `Vec::contains()`.
- **Type export**: add `#[derive(TS)]` + `#[ts(export)]` on models that need TypeScript types.
- **State access**: `State(state): State<AppState>` then `state.projects.read().await` / `.write().await`.
- **Linter**: `cargo clippy`
- **Formatter**: `cargo fmt`
- **Check**: `cargo check`

## TypeScript / React (frontend)

- **Node**: >= 23.6.0 (native TS support). Version managed via `fnm` or `.node-version`.
- **Target**: ES2020, strict mode.
- **Bundler**: Vite 5 with code splitting (React.lazy + Suspense, vendor chunks).
- **Icons**: Lucide React (`lucide-react`).
- **Types**: import from `../types/generated` — never define API types manually. Use `type` imports (`import type { ... }`).
- **API calls**: use functions from `../lib/api.ts` — never raw `fetch` in components.
- **Shared constants**: agent colors, labels, types → `lib/constants.ts`. Do not duplicate in pages.
- **Styling**: inline `style={{}}` objects. No CSS files, no Tailwind, no styled-components.
- **State**: local `useState` / `useEffect` / `useMemo` / `useCallback`. No global state library.
- **i18n**: use `useT()` hook from `I18nContext.tsx`. All user-visible strings must use `t('key.name')`. Translation keys in `lib/i18n.ts`. 3 UI locales: `fr`, `en`, `es`. Output languages (for agents) are separate and include `zh`, `br`.
- **Error boundaries**: wrap lazy-loaded routes with `ErrorBoundary` (see App.tsx).
- **SSE handling**: use `_streamSSE` helper in api.ts with `AbortController` for cancellation. Cleanup AbortControllers on component unmount.
- **Linter**: ESLint 10 (`npm run lint`) — strict config with typescript-eslint. 0 errors required.
- **Tests**: Vitest 4 (`npm test`, 124+ tests, 14 suites). Use @testing-library/react for component tests. Wrap state-triggering calls in `act()`.
- **Coverage**: `npm run test:coverage` — @vitest/coverage-v8 with text + lcov reporters.
- **Build**: `npm run build` (tsc + vite build)

## Shell scripts (lib/*.sh)

- **Compat**: Bash 3.2+ (macOS + Linux + WSL). No associative arrays, no `readarray`.
- **Portability**: detect GNU/BSD variants for `sed -i`, `cp -rn`, `timeout`.
- **Lint**: use `shellcheck` (not enforced yet, but recommended).
- **Tests**: bats-core (8 suites, 186 tests). Run with `make test-shell` or `bash tests/bats/run.sh`. Use `_load_lib()` helper from `test_helper.bash` to source scripts. All pure functions are tested; interactive functions (menus, agent install) are not testable in bats.

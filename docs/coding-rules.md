# Coding rules (AI contract)

> Glossary: `docs/glossary.md`.

## Global

- Prefer smallest diffs. Avoid drive-by refactors.
- Follow existing naming in adjacent code. Avoid generic names (`Helper`, `Utils`).
- No CSS framework. Styling via CSS tokens + utility classes + component classes (`src/styles/`). Inline `style={{}}` only for truly dynamic values (computed colors, transforms, animation).
- No emojis in code unless user explicitly requests them.
- **Comment sparingly — explain *why*, never *what*.** A comment earns its place only if it adds what the code can't say: a non-obvious rationale, a real gotcha, or a ticket ref for a *surprising* decision. Do NOT narrate what the code does, restate the line above, or leave "this is now handled by X — see Y" pointers the code/ticket already makes obvious. Match the file's existing comment density; a clear name beats a paragraph.

## Rust (backend)

- **Framework**: axum 0.8 with tokio async runtime.
- **Error handling**: `anyhow::Result` for internal, `ApiResponse` wrapper for HTTP.
- **Serialization**: serde with `#[serde(rename_all = "snake_case")]` on enums.
- **Route registration**: chain methods on same path — `.route("/path", get(h).post(h2))`, never two `.route()` calls with same path (axum panics).
- **Derive requirements**: add `PartialEq` to any enum used in `==` / `!=` / `Vec::contains()`.
- **Type export**: add `#[derive(TS)]` + `#[ts(export)]` on models that need TypeScript types.
- **State access**: `State(state): State<AppState>` then `state.projects.read().await` / `.write().await`.
- **Command execution**: ALWAYS use `crate::core::cmd::{async_cmd, sync_cmd}` instead of raw `tokio::process::Command::new()` or `std::process::Command::new()`. These helpers apply `CREATE_NO_WINDOW` on Windows (Tauri desktop). Raw `Command::new` causes visible console windows to flash.
- **WSL paths**: on Windows, detect WSL UNC paths (`\\wsl.localhost\...`) and run commands via `wsl.exe -e bash -lc "..."` (login shell needed for npm-installed binaries in PATH).
- **Linter**: `cargo clippy`
- **Formatter**: `cargo fmt`
- **Check**: `cargo check`

## TypeScript / React (frontend)

- **Node**: >= 23.6.0 (native TS support). Version managed via `fnm` or `.node-version`.
- **Target**: ES2020, strict mode.
- **Bundler**: Vite 8 with code splitting (React.lazy + Suspense, vendor chunks).
- **Icons**: Lucide React (`lucide-react`).
- **Types**: import from `../types/generated` — never define API types manually. Use `type` imports (`import type { ... }`).
- **API calls**: use functions from `../lib/api.ts` — never raw `fetch` in components.
- **Shared constants**: agent colors, labels, types → `lib/constants.ts`. Do not duplicate in pages.
- **Styling**: CSS tokens (`--kr-*`), utility classes (`.flex-row`, `.gap-*`, `.text-*`), and component classes (`.btn`, `.card`, `.input`, `.badge`) in `src/styles/`. Per-page CSS in `src/pages/*.css`. Inline `style={{}}` only for dynamic values. Import page CSS at the top: `import './PageName.css'`.
- **State**: local `useState` / `useEffect` / `useMemo` / `useCallback`. No global state library.
- **i18n**: use `useT()` from `I18nContext.tsx`. All user-visible strings must use `t('key.name')`. Locale files are separated under `lib/i18n/locales/`; `fr`, `en`, `es` and `zh` must keep exact key parity. The agent output language is an independent setting.
- **Error boundaries**: wrap lazy-loaded routes with `ErrorBoundary` (see App.tsx).
- **SSE handling**: use `_streamSSE` from `api.ts` with `AbortController` cancellation. Keep controller cleanup with the lifecycle owner; discussion streams intentionally survive page-tab switches and are cleaned on completion or explicit Stop.
- **Linter**: ESLint 10 (`pnpm lint`) is authoritative; Oxlint (`pnpm lint:fast`) is the zero-warning fast gate. ESLint requires zero errors and CI pins its warning count as a ratchet: cleanup lowers the budget, new warnings fail.
- **Tests**: Vitest 4 (`pnpm test`). Use Testing Library for component tests and wrap state-triggering calls in `act()`. Prefer `src/test/apiMock.ts` over inline API mock factories — see `docs/testing-quality.md`.
- **Coverage**: `pnpm test:coverage` — @vitest/coverage-v8 with enforced thresholds.
- **Build**: `pnpm build` (native TypeScript check + Vite build)

## Shell scripts (lib/*.sh)

- **Compat**: Bash 3.2+ (macOS + Linux + WSL). No associative arrays, no `readarray`.
- **Portability**: detect GNU/BSD variants for `sed -i`, `cp -rn`, `timeout`.
- **Lint**: use `shellcheck` (not enforced yet, but recommended).
- **Tests**: bats-core via `make test-shell` or `bash tests/bats/run.sh`. Use `_load_lib()` from `test_helper.bash` to source scripts. All pure functions are tested; interactive functions (menus, agent install) require a higher-level test.

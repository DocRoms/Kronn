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

- **Target**: ES2020, strict mode.
- **Bundler**: Vite.
- **Icons**: Lucide React (`lucide-react`).
- **Types**: import from `../types/generated` — never define API types manually.
- **API calls**: use functions from `../lib/api.ts` — never raw `fetch` in components.
- **Styling**: inline `style={{}}` objects. No CSS files, no Tailwind, no styled-components.
- **State**: local `useState` / `useEffect`. No global state library.
- **SSE handling**: use `_streamSSE` helper in api.ts with `AbortController` for cancellation.
- **Linter**: TypeScript compiler (`tsc --noEmit`)
- **Build**: `npm run build`

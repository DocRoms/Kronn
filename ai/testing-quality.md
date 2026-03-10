# Testing & quality (AI rules)

## Rules

- **Quality gate is non-negotiable**: code must compile and build after any change.
- **All tests must pass**: `npm run test` (frontend), `cargo test` (backend).
- **0 ESLint errors**: `npm run lint` must report 0 errors (warnings are tolerated for existing patterns).

## Build checks

| Layer | Command | Notes |
|-------|---------|-------|
| Rust compile | `cargo check` | Fast type check |
| Rust lint | `cargo clippy` | Must pass without warnings |
| Rust format | `cargo fmt --check` | Formatting check |
| TS compile | `cd frontend && npx tsc -b` | Type check |
| Frontend lint | `cd frontend && npm run lint` | ESLint 10 strict |
| Frontend tests | `cd frontend && npm test` | Vitest 4 (71 tests) |
| Frontend coverage | `cd frontend && npm run test:coverage` | Vitest + @vitest/coverage-v8 |
| Frontend build | `cd frontend && npm run build` | Production build (Vite, code-split) |
| Full stack | `make start` | Docker Compose build + up |

## Frontend test infrastructure

- **Runner**: Vitest 4 with happy-dom environment
- **Assertions**: @testing-library/react + @testing-library/jest-dom
- **Coverage**: @vitest/coverage-v8, reporters: text + lcov
- **Config**: `frontend/vite.config.ts` (test section)
- **Setup file**: `frontend/src/test/setup.ts`
- **Node requirement**: >= 23.6.0 (native TS support, latest tooling)

### Test files (9 suites, 71 tests)

| File | Tests | Covers |
|------|-------|--------|
| `src/lib/__tests__/i18n.test.ts` | 14 | Translations FR/EN/ES, interpolation, fallbacks, locale completeness |
| `src/lib/__tests__/constants.test.ts` | 9 | AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES, agentColor() |
| `src/lib/__tests__/api.test.ts` | 11 | GET/POST/DELETE requests, error handling (API error, 502 HTML, null error), API structure |
| `src/lib/__tests__/types.test.ts` | 10 | Generated types structure, union exhaustiveness, discriminated unions |
| `src/lib/__tests__/I18nContext.test.tsx` | 4 | I18nProvider, locale switching, persistence |
| `src/lib/__tests__/regression.test.ts` | 12 | Non-regression for all audit fixes (GeminiCli, constants, i18n, trigger_context, output languages) |
| `src/hooks/__tests__/useApi.test.ts` | 5 | Fetch on mount, errors, refetch, race condition protection |
| `src/__tests__/App.test.tsx` | 4 | Loading screen, SetupWizard vs Dashboard routing, API down fallback |
| `src/__tests__/ErrorBoundary.test.tsx` | 2 | Error catch + display, normal render |

### Coverage status

| Module | Stmts | Notes |
|--------|-------|-------|
| hooks/useApi.ts | 100% | Fully tested |
| lib/constants.ts | 100% | Fully tested |
| lib/i18n.ts | 100% | Fully tested |
| lib/I18nContext.tsx | 90% | Missing edge case |
| lib/api.ts | ~10% | SSE streams hard to unit test |
| pages/*.tsx | 0% | Require component extraction first |

### What's NOT tested

- **Shell scripts** (`lib/*.sh`): no test framework. Would need `bats-core` or `shellcheck` + integration tests.
- **Page components**: Dashboard.tsx (2250 lines), McpPage.tsx, WorkflowsPage.tsx, SetupWizard.tsx — monolithic, need extraction into smaller components before meaningful testing.
- **SSE streaming logic** in api.ts — requires mocking ReadableStream, complex setup.
- **Backend Rust**: `cargo test` only runs type generation. No API or unit tests yet.

## ESLint configuration

- **Config file**: `frontend/eslint.config.js` (ESLint 10 flat config)
- **Base**: typescript-eslint strict + react-hooks + react-refresh
- **Strict rules**: eqeqeq, prefer-const, no-var, no-throw-literal, no-implicit-coercion, consistent-type-imports
- **File-scoped overrides**: Dashboard.tsx (no-unused-expressions off — IIFE render blocks), api.ts (no-invalid-void-type off — `api<void>()`), generated.ts (no-explicit-any off), test files (lenient)

## Fast smoke checks

| Action | Command |
|--------|---------|
| Backend compiles | `cd backend && cargo check` |
| Frontend compiles | `cd frontend && npx tsc -b` |
| Frontend tests | `cd frontend && npm test` |
| Frontend lint | `cd frontend && npm run lint` |
| Type generation | `make typegen` |

## Troubleshooting (when command output is missing)

### Rule (critical)
- If output is missing, state it explicitly: **"I did not receive the command output"**.
- Retry once.
- If output is still missing, ask the user to **copy/paste the full command output** into the chat.

### Why
- Without the actual output, we cannot confirm PASS/FAIL or diagnose failures safely.

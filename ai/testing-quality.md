# Testing & quality (AI rules)

## Rules

- **Quality gate is non-negotiable**: code must compile and build after any change.
- **All tests must pass**: `npm run test` (frontend, **489 tests**), `cargo test` (backend, **1166 tests**: 1032 lib + 134 integration), `make test-shell` (192 bats tests).
- **0 ESLint errors**: `npm run lint` must report 0 errors (warnings are tolerated for existing patterns).
- **0 clippy warnings**: `cargo clippy --all-targets -- -D warnings` must pass.

## Build checks

| Layer | Command | Notes |
|-------|---------|-------|
| Rust compile | `cargo check` | Fast type check |
| Rust lint | `cargo clippy` | Must pass without warnings |
| Rust format | `cargo fmt --check` | Formatting check |
| TS compile | `cd frontend && npx tsc -b` | Type check |
| Frontend lint | `cd frontend && npm run lint` | ESLint 10 strict |
| Frontend tests | `cd frontend && npm test` | Vitest 4 (489 tests) |
| Frontend coverage | `cd frontend && npm run test:coverage` | Vitest + @vitest/coverage-v8 |
| Frontend build | `cd frontend && npm run build` | Production build (Vite, code-split) |
| Shell tests | `make test-shell` | bats-core (186 tests) |
| Full stack | `make start` | Docker Compose build + up |

## Frontend test infrastructure

- **Runner**: Vitest 4 with happy-dom environment
- **Assertions**: @testing-library/react + @testing-library/jest-dom
- **Coverage**: @vitest/coverage-v8, reporters: text + lcov
- **Config**: `frontend/vite.config.ts` (test section)
- **Setup file**: `frontend/src/test/setup.ts`
- **Node requirement**: >= 23.6.0 (native TS support, latest tooling)

### Frontend testing patterns (0.3.5)

- **Shared API mock**: `src/test/apiMock.ts` exposes `buildApiMock(overrides)`. Factory covers all 13 namespaces + 5 flat fns of `lib/api.ts`. Deep-merges overrides namespace-by-namespace so slim overrides don't wipe sibling methods. **Always use via `vi.hoisted` + `vi.mock`** — factory refs at the top level break because `vi.mock` is hoisted above imports.
- **Completeness guard**: `src/test/apiMock.complete.test.ts` imports the real `lib/api.ts` and asserts every top-level export is covered. Adding a new namespace without updating the default mock fails this test.
- **i18n parity**: `src/lib/__tests__/i18n-parity.test.ts` — imports the exported `dictionaries` object and asserts fr/en/es key set isomorphism + non-empty values + placeholder-subset invariant (en/es may have fewer `{N}` than fr, never extras).
- **Module-level state trackers**: patterns like `activeStepTests` (Map + subscribe/notify) in `WorkflowDetail.tsx` survive React unmount. Tests using these must either render inside the parent or mock the tracker directly.

### Test files (37 suites, 489 tests)

| File | Tests | Covers |
|------|-------|--------|
| `src/lib/__tests__/i18n.test.ts` | 14 | Translations FR/EN/ES, interpolation, fallbacks, locale completeness |
| `src/lib/__tests__/constants.test.ts` | 9 | AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES (6 agents incl. CopilotCli), agentColor() |
| `src/lib/__tests__/api.test.ts` | 11 | GET/POST/DELETE requests, error handling (API error, 502 HTML, null error), API structure |
| `src/lib/__tests__/types.test.ts` | 10 | Generated types structure, union exhaustiveness, discriminated unions |
| `src/lib/__tests__/I18nContext.test.tsx` | 4 | I18nProvider, locale switching, persistence |
| `src/lib/__tests__/regression.test.ts` | 12 | Non-regression for all audit fixes (GeminiCli, constants, i18n, trigger_context, output languages) |
| `src/lib/__tests__/access-warnings.test.ts` | 41 | i18n keys for access warnings (all locales), checkAgentRestricted, hasFullAccess, isAgentDisabled |
| `src/hooks/__tests__/useApi.test.ts` | 5 | Fetch on mount, errors, refetch, race condition protection |
| `src/__tests__/App.test.tsx` | 4 | Loading screen, SetupWizard vs Dashboard routing, API down fallback |
| `src/__tests__/ErrorBoundary.test.tsx` | 2 | Error catch + display, normal render |
| `src/pages/__tests__/WorkflowsPage.test.tsx` | 3 | Render with undefined/restricted/full agentAccess |
| `src/pages/__tests__/DiscussionsPage.test.tsx` | 26 | Render, prefill, sidebar (message_count, titles, archives, org groups, collapse, search filter), streaming (thinking loader, tab restore, SSE abort, refetch), TTS (toggle, persist, play, speech cancel), discussion creation, copy button, response time, overflow-wrap, agent switch (button, dropdown) |
| `src/pages/__tests__/SettingsPage.test.tsx` | 13 | Render, agents config, scan sections, API key management, model tiers, Usage section nav + filter buttons |
| `src/pages/__tests__/McpPage.test.tsx` | 3 | Render with minimal props, configs, agents |
| `src/lib/__tests__/agent-question-parse.test.ts` | 15 | Structured agent question parser: {{var}}: question extraction, edge cases, multi-var |
| `src/components/__tests__/AgentQuestionForm.test.tsx` | 5 | AgentQuestionForm render, field filling, submit, empty state, integration with parser |

### Coverage status

| Module | Stmts | Notes |
|--------|-------|-------|
| hooks/useApi.ts | 100% | Fully tested |
| lib/constants.ts | 100% | Fully tested |
| lib/i18n.ts | 100% | Fully tested |
| lib/I18nContext.tsx | 90% | Missing edge case |
| lib/api.ts | ~10% | SSE streams hard to unit test |
| pages/*.tsx | ~5% | Basic render tests for 4 pages (Workflows, Discussions, Settings, MCP) |

### Shell test infrastructure

- **Runner**: bats-core with bats-assert + bats-support (git submodules in `tests/bats/`)
- **Test runner**: `make test-shell` or `bash tests/bats/run.sh`
- **Helper**: `tests/bats/test_helper.bash` — `_load_lib()` function, pre-initialized color variables

### Shell test files (8 suites, 192 tests)

| File | Tests | Covers |
|------|-------|--------|
| `tests/bats/agents.bats` | 42 | `_parse_version`, `_agent_idx` (6 agents incl. kiro-cli + copilot), `_count_detected` (0-6), `_format_agent_line`, `_check_node_version`, Kiro + Copilot metadata |
| `tests/bats/mcps.bats` | 19 | `secret_get` (TOML parsing), `init_secrets` (creation, perms, idempotence), `secrets_configured` |
| `tests/bats/tron.bats` | 32 | `_tron_format_elapsed`, `_tron_pad`, `tron_init/cleanup`, `tron_progress`, `tron_set_step/log/agent`, `tron_signal_done`, progress file in target dir, `_tron_write_progress_file` |
| `tests/bats/ui.bats` | 28 | `info/success/warn/fail/step/banner`, color variables, return codes, empty messages, special characters |
| `tests/bats/repos.bats` | 24 | `scan_repos` (find/names/status/empty/nested/reset/default), `detect_ai_context` (12 cases), `ensure_gitignore` |
| `tests/bats/analyze.bats` | 16 | `inject/has/remove_bootstrap_prompt`, roundtrip, `_ANALYSIS_STEPS` validation, marker constants |
| `tests/bats/portability.bats` | 18 | `_safe_timeout`, `remove_bootstrap_prompt` (GNU/BSD sed), `ensure_gitignore`, `detect_ai_context`, `scan_repos`, rsync/cp fallback |
| `tests/bats/bugfixes.bats` | 12 | Non-regression: cross-filesystem temp, sed delimiter, KRONN_DIR guard, envsubst leak, gitignore guard |

### What's NOT tested

- **Page components**: Dashboard.tsx (~650 lines), SetupWizard.tsx — basic render tests exist for 4 sub-pages but deeper interaction/state tests still needed.
- **SSE streaming logic** in api.ts — requires mocking ReadableStream, complex setup.
- **Backend Rust**: **1166 tests** (1032 lib + 134 integration). Key test suites: `discussions_test.rs` (21 tests: CRUD, archive, title editing, message management, AgentType round-trip for all 6 agents, DB string stability), `runner_test.rs` (agent commands, model tiers, token parsing, stream parsing for all agents), `pricing.rs` (cost estimation for all 6 providers), `key_discovery.rs` (cross-platform HOME resolution), `mod.rs` (agent detection with .cmd/.exe extensions, WSL_DISTRO_NAME detection), `env.rs` (is_docker, host_os_label), `scanner.rs` (shellexpand ~/, UNC paths), `db/tests.rs` (partial_response set/recover/idempotency + `partial_response_started_at` preservation + `has_pending_partial`), `tests/api_tests.rs` HTTP integration: dismiss-partial + WS broadcast, partial_pending guard on send_message, boot recovery simulation, workflow_cancel_run cascade to child discs via parent_run_id + idempotent on finished run. Run with `cargo test`.
- **Shell interactive functions**: menu systems, agent installation/uninstall, terminal animation (require terminal I/O, tested indirectly via non-interactive helpers).

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
| Shell tests | `make test-shell` |
| Type generation | `make typegen` |

## Troubleshooting (when command output is missing)

### Rule (critical)
- If output is missing, state it explicitly: **"I did not receive the command output"**.
- Retry once.
- If output is still missing, ask the user to **copy/paste the full command output** into the chat.

### Why
- Without the actual output, we cannot confirm PASS/FAIL or diagnose failures safely.

# Project documentation index — Single entry point

**Project:** Kronn — Self-hosted CLI + web UI for managing AI coding agents (Claude Code, Codex, Vibe, Gemini CLI, Kiro) across git repositories. Unified workflow engine for cron, multi-step pipelines, tracker-driven automation, and manual triggers.

> **All files under `docs/` are in English by default.** Project documentation must be written in English.
> **ATTENTION — This is the reference file for all AI agents.**
> Read this file first, then follow the context loading strategy below.
> Do not read the other config files (.cursorrules, copilot-instructions, etc.) — they redirect here.

> **CRITICAL — Never hallucinate.**
> - **Never invent information** (tech stack, conventions, architecture, file paths...).
> - If you are unsure about something: **check the `docs/` documentation first**.
> - If you still don't find the answer: **ask the user** rather than guessing.
> - After getting the answer: **update the relevant `docs/` file** so the knowledge is captured.
> - Getting it right matters more than answering fast — hallucinations waste everyone's time.

> **CRITICAL — MCP tool usage.**
> Before calling any MCP tool, **read the matching context file** in `docs/operations/mcp-servers/<mcp-name>.md` **if it exists**.
> These files contain project-specific rules, constraints, and examples that prevent hallucinations and misuse.
> If no context file exists for an MCP, **proceed normally** — do not block on a missing doc.
> (Each MCP tool ships its own `description` field via JSON-RPC `tools/list` ; that's the in-band contract. Context files are an additional, optional layer of project-specific guidance.)

> **Discussion notes are routing metadata, not secrets.**
> Messages on the `note` channel stay visible to humans but are excluded from
> ordinary agent context and room delivery. Read them only on explicit request
> through the bounded `disc_note_list` tool; see
> [`operations/mcp-servers/kronn-internal.md`](operations/mcp-servers/kronn-internal.md#out-of-context-discussion-notes).

**Unknown term?** → `docs/glossary.md` first.

This folder (`docs/`) contains structured project context (for both humans and AI agents). Use paths relative to repo root.

<!-- kronn:section name="anti-hallu" curated="ai" audit="2026-05-27" -->
## 0. Anti-Hallucination Protocol

You may NEVER state a non-trivial technical fact (file paths, function / API / config names, versions, behaviour, conventions) without proof. Apply this cascade — stop as soon as you have it:

1. **READ THE CODE** — Read / Glob / Grep the repo. Cite `file:line`. Source of truth #1.
2. **READ `docs/`** — siblings of this file, `conventions/`, `architecture/`, etc. Trust a doc claim only if its `[src:]` still resolves.
3. **OFFICIAL EXTERNAL DOC** — WebFetch / the relevant MCP for external libs / APIs / specs. Cite the URL.
4. **ASK THE USER** — directly, or via a focused sub-discussion. Faster than guessing.
5. **NEVER ASSERT WITHOUT PROOF** — "I don't know yet, let me check" beats a fabrication every time.

### Citation grammar (verified mechanically by Kronn when present)

Attach a structured citation to every non-trivial assertion:

- `[src: file: <path>:<line>]` — e.g. `[src: file: backend/src/lib.rs:440]`
- `[src: file: <path>:<start-end>]` — line range
- `[src: url: <url>]` — external doc
- `[src: user: <YYYY-MM-DD>: <ref>]` — human confirmation
- `[src: commit: <sha>]` — git commit

A citation pointing to a file/line that does not exist, or escaping the project root, is **rejected as fabricated**. A code comment is NOT authoritative — treat it as a hint to verify, never as the fact itself.

Full spec: [`docs/conventions/agents-md-format-v1.md`](conventions/agents-md-format-v1.md). **Honest by design**: `verified` means the citation *exists*, not that the claim is *true*.
<!-- kronn:section:end -->

---

## 1. Entry procedure (mandatory)

### Tiered context loading strategy

#### Tier 1 — Always read
- `docs/AGENTS.md` (this file)

**Trivial tasks** (typos, config tweaks, simple style fixes): Tier 1 may suffice.

#### Common tasks — load exactly these files

| Task | Files to load |
|------|---------------|
| Backend API changes | `docs/repo-map.md`, `docs/coding-rules.md` |
| Frontend UI changes | `docs/repo-map.md`, `docs/coding-rules.md`, `docs/architecture/ui-structure.md` |
| Add new API endpoint | `docs/repo-map.md`, `docs/architecture/overview.md` |
| RTK / compression integration | `docs/architecture/rtk-integration.md` |
| Workflow engine work | `docs/architecture/overview.md`, `docs/inconsistencies-tech-debt.md`, `docs/coding-rules.md` |
| Docker / deployment / starting the stack | `docs/operations/debug-operations.md`, `docs/operations/running-the-stack.md` |
| Secret themes / unlock features | `docs/operations/secret-themes.md` |
| **Désagentification / `ApiCall` step** (workflow engine calls APIs directly, zero tokens) — incl. AI helper bubble | `docs/operations/deagent-apicall.md` |
| **Ollama local models** (deterministic offload: model resolution, num_ctx / `/no_think` gotchas, TypedSchema `format`, quality escalation) | `docs/operations/ollama-local-models.md` |
| Token cost | `docs/operations/token-economy-0.9.6.md` |
| Fix known issue | `docs/inconsistencies-tech-debt.md` |

#### Tier 2 — For needs not covered above (max 3 files)

| Need | File |
|------|------|
| repo structure / code placement | `docs/repo-map.md` |
| testing / quality | `docs/testing-quality.md` |
| coding rules | `docs/coding-rules.md` |
| known issues / tech debt | `docs/inconsistencies-tech-debt.md` |
| Architecture decisions | `docs/decisions.md` |
| term definitions / project jargon | `docs/glossary.md` |
| dependency versions per layer | `docs/stack.md` |

#### Tier 3 — Escalation
Only if Tier 1 + 2 are insufficient: state which file you need and why, read it, or ask the user.
Never load everything "just in case".
- Architecture overview → `docs/architecture/overview.md`

---

## 2. Running the stack

→ [`operations/running-the-stack.md`](operations/running-the-stack.md)

## 3. DO NOT (common mistakes)

- Do **not** guess when information is missing — ask the user.
- Do **not** load all Tier 2 files at once — pick up to 3 max.
- Do **not** modify business code when the task is only about project documentation — edit `docs/` only.
- Do **not** edit `frontend/src/types/generated.ts` by hand — run `make typegen`.
- Do **not** register two `.route()` calls with the same path in axum — chain methods: `.route("/path", get(h1).post(h2))`.
- Do **not** forget `#[derive(PartialEq)]` on enums used in comparisons (`AgentType`, `MessageRole`).
- Do **not** use the old axum 0.7 path-param syntax `:id` — axum 0.8 expects `{id}`. Failure to migrate means the route panics on registration.
- Do **not** wrap an extractor in `Option<Query<…>>` or `Option<ConnectInfo<…>>` — axum 0.8 dropped `OptionalFromRequestParts` for those. Use the concrete extractor with a sentinel default (`Query<…>` with `page=0` "no pagination") or wrap via `Option<Extension<…>>`.
- Do **not** slice `&str` with a hard-coded byte index (`&s[..N]`) when truncating user/agent text — UTF-8 (French, emoji, accented filenames) panics at non-boundary bytes. Use `s.chars().take(N).collect::<String>()` instead. Pattern documented in `feedback_rust_str_slicing` memory.
- Do **not** rely on `disabled={state}` alone to gate an async button handler — React's state update is async, so two synchronous clicks read the stale closure and fire two API calls. Use `useRef` + check at top of handler. Helper: `useAsyncGuard` in `frontend/src/hooks/useAsyncGuard.ts`.
- Do **not** nest a `<button>` inside another `<button>` — invalid HTML and produces a React dev warning. Convert the outer to `<div role="button" tabIndex={0}>` with explicit `onKeyDown` for Enter/Space.
---

## 4. Development constraints

- **Docker-first**: the full app runs via `docker compose`. Backend, frontend, and gateway are separate services.
- **Quality is mandatory**: `cargo clippy -- -D warnings` must pass. Frontend: `npx tsc --noEmit` + `pnpm test`. Shell: `make test-shell`. E2E: `pnpm test:e2e` (Playwright; backend must be running, Vite is auto-spawned). The `frontend/e2e/` tree has its own README — fixtures + page objects + 35+ specs covering the smoke surface, the wizard, the guided tour, MCP host-discovery, and the QP launch double-click race.
- **Release versions are mechanically synchronized**: use `make bump V=x.y.z`,
  then `make check-version` before tagging. The guard compares `VERSION`, app
  manifests and lockfiles, the first changelog release, both READMEs (including
  clone commands), and the FR/EN/ES public site; CI runs the same check.
  `[src: file: Makefile:417-460]`
  `[src: file: scripts/check-version-sync.sh:1-106]`
- **Type generation**: Rust models are the source of truth. TypeScript types are auto-generated via `ts-rs`.
- If stdout/stderr is missing: ask the user to copy/paste the full output.

### Testing rule (mandatory)

**Every code change MUST include tests.** This is not optional — tests are the primary defense against regressions and AI hallucinations.

| Change type | Required tests |
|-------------|---------------|
| New API endpoint | Integration test in `backend/tests/api_tests.rs` (HTTP request → response assertion) |
| New backend function | Unit test in same file (`#[cfg(test)] mod tests`) |
| Bug fix | Regression test proving the bug is fixed (test fails without fix, passes with) |
| New frontend component | Test file in `__tests__/` (render + key interactions) |
| Frontend behavior change | Update existing tests + add edge case coverage |
| Database migration | Verify migration applies cleanly in existing DB tests |

**Test quality rules:**
- Test **behavior**, not implementation details.
- Include **edge cases**: empty input, large input, unicode, error paths.
- Assertions must be **meaningful** — not just "renders without crashing".
- Mocks must match **real API shapes** (check `types/generated.ts`).
- Run `cargo test` (backend) and `npx vitest run` (frontend) **before declaring a task done**.
- If a test is flaky, fix the root cause — do not add retries or sleeps.

**Why this matters:** A failing test catches a bug in seconds. Without tests, bugs surface in production, require debugging, and cost 10-100x more tokens to fix. Tests also prove to the user that the code works — "all 500 tests pass" is more convincing than "I think it's correct".

---

## 5. Source of truth

- Project documentation: `docs/AGENTS.md` (this file) + `~/.kronn/user-context/` for personal/role-specific overrides. The legacy `ai/` directory was migrated to `docs/` in 0.7.1 (kept here as historical note; tooling no longer reads from `ai/`).
- Rust data models: `backend/src/models/mod.rs`.
- TypeScript types: `frontend/src/types/generated.ts` (auto-generated from Rust).
- API routes: `backend/src/lib.rs` (router definition in `build_router()`).
- Database schema: `backend/src/db/sql/001_initial.sql` plus the ordered
  registry in `backend/src/db/migrations.rs`.
- Docker config: `docker-compose.yml`.

---

## 6. Code placement

Use `docs/repo-map.md` to decide.
- New API endpoints: add handler in `backend/src/api/<domain>.rs`, register route in `backend/src/lib.rs` (`build_router()`).
- Workflow engine code: `backend/src/workflows/`.
- New frontend pages: `frontend/src/pages/`.
- New hooks: `frontend/src/hooks/`.
- API client functions: `frontend/src/lib/api.ts`.
- Data models: `backend/src/models/mod.rs` (+ `make typegen`).

---

## 7. Code generation (critical behavior)

- Search the repo for similar implementations before writing.
- Use `docs/repo-map.md` to decide where code goes.
- If info is missing or ambiguous: ask questions; do not guess.
- If a "logical fix" requires a large/risky refactor: add an entry to `docs/inconsistencies-tech-debt.md`.
- **Write tests for every change** — see § 4 Testing rule. No exceptions.

### Project documentation maintenance rule
After completing a task: if you discovered something non-obvious (a gotcha, a missing pattern, an outdated doc), update the relevant `docs/` file before closing. Keep entries factual and concise.

---

## 8. Stack

→ [`stack.md`](stack.md)

## 9. UI structure

→ [`architecture/ui-structure.md`](architecture/ui-structure.md)

## 9bis. Media generation (image / video)

→ [`architecture/media-generation.md`](architecture/media-generation.md)

Read it before touching a media path: submission is billable and the rules
around resubmission, provider URLs and cost persistence exist to keep a crash
from paying twice.

---

## 10. RTK integration

Kronn's RTK detection/activation internals moved to
[`docs/architecture/rtk-integration.md`](architecture/rtk-integration.md).
The rule for agents — prefix commands with `rtk` — is in `CLAUDE.md`.

## 11. Multi-agent configuration

Redirectors to this file: `CLAUDE.md`, `GEMINI.md`, `AGENTS.md`, `.kiro/steering/instructions.md`, `.vibe/instructions.md`, `.cursorrules`, `.cursor/rules/repo-instructions.mdc`, `.github/copilot-instructions.md`, `.windsurfrules`, `.clinerules`.

**Maintenance rule**: all content lives in `docs/`. Redirectors contain a summary of critical rules + pointer to `docs/AGENTS.md` as source of truth.

---

## 12. Documentation history

→ [`release-notes-archive.md`](release-notes-archive.md)

<!-- kronn:section name="learnings" curated="ai" -->
## Learned conventions

Validated learnings accumulate in [`docs/learnings.md`](learnings.md). Load it when a task touches project conventions, preferences, or known pitfalls.
<!-- kronn:section:end -->

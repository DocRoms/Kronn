# AI context index — Single entry point

**Project:** Kronn — Self-hosted CLI + web UI for managing AI coding agents (Claude Code, Codex, Vibe, Gemini CLI, Kiro) across git repositories. Unified workflow engine for cron, multi-step pipelines, tracker-driven automation, and manual triggers.

> **All files under `ai/` are in English by default.** AI context documentation must be written in English.
> **ATTENTION — This is the reference file for all AI agents.**
> Read this file first, then follow the context loading strategy below.
> Do not read the other config files (.cursorrules, copilot-instructions, etc.) — they redirect here.

> **CRITICAL — Never hallucinate.**
> - **Never invent information** (tech stack, conventions, architecture, file paths...).
> - If you are unsure about something: **check the `ai/` documentation first**.
> - If you still don't find the answer: **ask the user** rather than guessing.
> - After getting the answer: **update the relevant `ai/` file** so the knowledge is captured.
> - Getting it right matters more than answering fast — hallucinations waste everyone's time.

> **CRITICAL — MCP tool usage.**
> Before calling any MCP tool, **read the matching context file** in `ai/operations/mcp-servers/<mcp-name>.md`.
> These files contain project-specific rules, constraints, and examples that prevent hallucinations and misuse.
> If no context file exists for an MCP you need to use, ask the user before proceeding.

**Unknown term?** → `ai/glossary.md` first.

This folder (`ai/`) contains AI-optimized project context (not human docs). Use paths relative to repo root.

---

## 1. Entry procedure (mandatory)

### Tiered context loading strategy

#### Tier 1 — Always read
- `ai/index.md` (this file)

**Trivial tasks** (typos, config tweaks, simple style fixes): Tier 1 may suffice.

#### Common tasks — load exactly these files

| Task | Files to load |
|------|---------------|
| Backend API changes | `ai/repo-map.md`, `ai/coding-rules.md` |
| Frontend UI changes | `ai/repo-map.md`, `ai/coding-rules.md` |
| Add new API endpoint | `ai/repo-map.md`, `ai/architecture/overview.md` |
| Workflow engine work | `ai/architecture/overview.md`, `ai/inconsistencies-tech-debt.md`, `ai/coding-rules.md` |
| Docker / deployment | `ai/operations/debug-operations.md` |
| Fix known issue | `ai/inconsistencies-tech-debt.md` |

#### Tier 2 — For needs not covered above (max 3 files)

| Need | File |
|------|------|
| repo structure / code placement | `ai/repo-map.md` |
| testing / quality | `ai/testing-quality.md` |
| coding rules | `ai/coding-rules.md` |
| known issues / tech debt | `ai/inconsistencies-tech-debt.md` |
| Architecture decisions | `ai/decisions.md` |
| term definitions / project jargon | `ai/glossary.md` |

#### Tier 3 — Escalation
Only if Tier 1 + 2 are insufficient: state which file you need and why, read it, or ask the user.
Never load everything "just in case".
- Architecture overview → `ai/architecture/overview.md`

---

## 2. Prerequisites before running commands

- **Docker + Docker Compose** required for running the full stack.
- Start: `./kronn start` or `make start` (builds and runs all services).
- Stop: `./kronn stop` or `make stop`.
- Restart: `./kronn restart`.
- Logs: `./kronn logs` or `make logs`.
- Dev backend only: `make dev-backend` (cargo watch with auto-reload).
- Dev frontend only: `make dev-frontend` (Vite dev server on :5173).
- After changing Rust models with `#[derive(TS)]`: run `make typegen` to regenerate `frontend/src/types/generated.ts`.
- After `docker compose build`, always restart the gateway: `docker compose restart gateway` or `docker compose down && docker compose up -d`.

---

## 3. DO NOT (common mistakes)

- Do **not** guess when information is missing — ask the user.
- Do **not** load all Tier 2 files at once — pick up to 3 max.
- Do **not** modify business code when the task is only about AI context — edit `ai/` only.
- Do **not** edit `frontend/src/types/generated.ts` by hand — run `make typegen`.
- Do **not** register two `.route()` calls with the same path in axum — chain methods: `.route("/path", get(h1).post(h2))`.
- Do **not** forget `#[derive(PartialEq)]` on enums used in comparisons (`AgentType`, `MessageRole`).

---

## 4. Development constraints

- **Docker-first**: the full app runs via `docker compose`. Backend, frontend, and gateway are separate services.
- **Quality is mandatory**: `cargo clippy -- -D warnings` must pass. Frontend: `npx tsc --noEmit` + `pnpm test`. Shell: `make test-shell`.
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

- AI context: `ai/`.
- Rust data models: `backend/src/models/mod.rs`.
- TypeScript types: `frontend/src/types/generated.ts` (auto-generated from Rust).
- API routes: `backend/src/lib.rs` (router definition in `build_router()`).
- Database schema: `backend/src/db/sql/001_initial.sql` (+ migrations 002-017).
- Docker config: `docker-compose.yml`.

---

## 6. Code placement

Use `ai/repo-map.md` to decide.
- New API endpoints: add handler in `backend/src/api/<domain>.rs`, register route in `backend/src/lib.rs` (`build_router()`).
- Workflow engine code: `backend/src/workflows/`.
- New frontend pages: `frontend/src/pages/`.
- New hooks: `frontend/src/hooks/`.
- API client functions: `frontend/src/lib/api.ts`.
- Data models: `backend/src/models/mod.rs` (+ `make typegen`).

---

## 7. Code generation (critical behavior)

- Search the repo for similar implementations before writing.
- Use `ai/repo-map.md` to decide where code goes.
- If info is missing or ambiguous: ask questions; do not guess.
- If a "logical fix" requires a large/risky refactor: add an entry to `ai/inconsistencies-tech-debt.md`.
- **Write tests for every change** — see § 4 Testing rule. No exceptions.

### AI context maintenance rule
After completing a task: if you discovered something non-obvious (a gotcha, a missing pattern, an outdated doc), update the relevant `ai/` file before closing. Keep entries factual and concise.

---

## 8. Stack (facts)

| Layer | Technology |
|-------|------------|
| Backend | Rust (axum 0.7, tokio, serde, anyhow) |
| Frontend | React 18 + TypeScript (Vite 5, Lucide icons, Node >= 24 LTS) |
| Styling | Inline styles (no CSS framework) |
| i18n | Custom lightweight system (fr/en/es), localStorage, no external lib |
| Type bridge | ts-rs (Rust → TypeScript) |
| Database | SQLite (`kronn.db`, WAL mode, foreign keys) |
| Streaming | SSE (Server-Sent Events) for agent responses and workflow run updates |
| Container | Docker Compose (backend + frontend + nginx gateway) |
| Agents | Claude Code CLI, OpenAI Codex CLI, Vibe (Mistral), Gemini CLI (Google), Kiro (Amazon). Planned: OpenCode, DeepSeek |
| MCP sync | 6 formats: `.mcp.json` (Claude), `.kiro/settings/mcp.json` (Kiro), `.ai/mcp/mcp.json` (Kiro new), `.gemini/settings.json` (Gemini CLI), `.vibe/config.toml` (Vibe), `~/.codex/config.toml` (Codex) |
| API keys | Multi-key per provider (named keys, active selection), stored in `config.toml` as `[[tokens.keys]]` array. Agent auth files synced (e.g. `~/.codex/auth.json`). Override toggle per provider without deleting keys. |
| Token tracking | Per-message `tokens_used` + `auth_mode` (override/local). Codex: parsed from stderr. Claude Code: `--output-format stream-json --verbose --include-partial-messages` (tokens from `result` event and `message_delta`). Gemini/Vibe: TODO. |

---

## 9. UI structure

Dashboard tabs (current / planned):

| Tab | Status | Content |
|-----|--------|---------|
| Projets | Done | Project list, AI audit pipeline (template → audit → validation), project bootstrap (create from scratch), MCP overview, per-project workflows/skills/doc viewer |
| Discussions | Done | Single/multi-agent chat, @mentions, orchestration, global discussions, archive/unarchive (swipe gestures), inline title editing, disabled agent detection |
| MCPs | Done | MCP registry and management |
| Workflows | Done | Workflow list (grouped by project), creation wizard (5-step: infos → trigger → steps → config → resume), detail + runs with live SSE progress, manual trigger, run deletion (individual + bulk). MCP tools auto-injected into agent prompts. Symphony import planned. |
| Config | Done | Multi-key API management, token usage tracking, language, agent detection + permissions, Skills/Profiles/Directives CRUD with live cards, DB management (export/import) |

Note: the old "Agents" tab has been merged into Config. Nav order: Projets → Discussions → MCPs → Workflows → Config.

### Project Bootstrap (create from scratch)

`POST /api/projects/bootstrap` — creates a new project directory, initializes git, installs AI template, creates a bootstrap discussion with architect + product-owner profiles. The discussion prompt guides the AI through: Vision → Architecture → Structure → MVP → Action Plan. Frontend modal accessible via "New project" button in nav bar. Parent directory determined from existing projects' common parent or `KRONN_REPOS_DIR` env var.

### CI pipeline

GitHub Actions workflow (`.github/workflows/ci-test.yml`) triggered on push to `main` + all PRs:
- `test-backend`: cargo clippy + cargo test (with sccache)
- `test-frontend`: tsc --noEmit + pnpm test (Node 24 LTS)
- `test-shell`: make test-shell (bats)
- `security-scan`: cargo audit + pnpm audit

### AI audit pipeline (4-state badge system)

Projects display 3 badges next to the title: `[FileCode] AI context`, `[Cpu] AI audit`, `[ShieldCheck] Validated`.

| State | AI context | AI audit | Validated | Meaning |
|-------|-----------|----------|-----------|---------|
| NoTemplate | gray | gray | hidden | No `ai/` directory |
| TemplateInstalled | green | orange | gray | Template copied, audit pending |
| Audited | green | green | gray | 10-step audit completed |
| Validated | green | green | green | Validation discussion resolved all TODOs |

- **Template install**: copies `ai/` skeleton + redirector files (CLAUDE.md, .cursorrules, etc.) + injects bootstrap prompt
- **AI audit**: 10-step SSE streaming, ~20 min. **Token cost: ~50K–150K tokens per audit** (depends on project size and agent model). With Claude Sonnet API pricing, expect ~$0.50–$2.00 per audit. Fills all `ai/` files.
- **Validation**: opens a prefilled discussion (locked title/prompt) where the AI asks questions about ambiguities. AI updates `ai/` files after each answer. Project page shows "validation en cours" + link to discussion (no validate button on project page).
- When the AI finishes all questions, it includes `KRONN:VALIDATION_COMPLETE` in its last message. This triggers a green banner in the discussion with a "Marquer l'audit comme valide" button.
- **Mark as validated**: injects `<!-- KRONN:VALIDATED:date -->` marker into `ai/index.md`.
- AI config file badges (CLAUDE.md, .cursorrules, etc.) shown on a second line below the status badges.

---

## 10. Multi-agent configuration

Redirectors to this file: `CLAUDE.md`, `GEMINI.md`, `AGENTS.md`, `.kiro/steering/instructions.md`, `.vibe/instructions.md`, `.cursorrules`, `.cursor/rules/repo-instructions.mdc`, `.github/copilot-instructions.md`, `.windsurfrules`, `.clinerules`.

**Maintenance rule**: all content lives in `ai/`. Redirectors contain a summary of critical rules + pointer to `ai/index.md` as source of truth.

---

## 11. Last updated

AI context last reviewed: **2026-03-14**.

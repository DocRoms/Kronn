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
| Secret themes / unlock-gated features | `ai/operations/secret-themes.md` |
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
- **Windows**: WSL2 is mandatory — Kronn mounts host binaries, Unix sockets, and Linux paths. Docker Desktop is not required; Docker Engine inside WSL works. Install WSL: `wsl --install`.
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
- Database schema: `backend/src/db/sql/001_initial.sql` (+ migrations 002-019).
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
| Styling | CSS tokens + utility classes + component classes (`src/styles/`). Inline `style={{}}` only for dynamic values. No CSS framework |
| i18n | Custom lightweight system (fr/en/es), localStorage, no external lib |
| Type bridge | ts-rs (Rust → TypeScript) |
| Database | SQLite (`kronn.db`, WAL mode, foreign keys) |
| Streaming | SSE (Server-Sent Events) for agent responses and workflow run updates |
| Container | Docker Compose (backend + frontend + nginx gateway) |
| Agents | Claude Code CLI, OpenAI Codex CLI, Vibe (Mistral), Gemini CLI (Google), Kiro (Amazon), **Ollama (local, v0.4.0)** — HTTP API streaming `/api/chat` with system/user role separation, zero cost. Planned: OpenCode, DeepSeek |
| MCP sync | 7 formats: `.mcp.json` (Claude), `.kiro/settings/mcp.json` (Kiro), `.ai/mcp/mcp.json` (Kiro new), `.gemini/settings.json` (Gemini CLI), `.vibe/config.toml` (Vibe), `~/.codex/config.toml` (Codex), `~/.copilot/mcp-config.json` (Copilot CLI). Also syncs Claude Code's `.claude/settings.local.json` `enabledMcpjsonServers` whitelist |
| Skills sync | Native SKILL.md files written to `.claude/skills/`, `.agents/skills/` (Codex), `.gemini/skills/` for progressive agent discovery. Profiles synced as agent files (`.claude/agents/`, `.gemini/agents/`, `.codex/agents/`, `.copilot/agents/`). Vibe/Kiro: prompt injection fallback |
| API keys | Multi-key per provider (named keys, active selection), stored in `config.toml` as `[[tokens.keys]]` array. Agent auth files synced (e.g. `~/.codex/auth.json`). Override toggle per provider without deleting keys. |
| Token tracking | Per-message `tokens_used` + `auth_mode` (override/local). Codex: parsed from stderr. Claude Code: `--output-format stream-json --verbose --include-partial-messages` (tokens from `result` event and `message_delta`). Ollama: `prompt_eval_count` + `eval_count` from streaming JSON `done` chunk (cost: $0). Gemini/Vibe: TODO. |

---

## 9. UI structure

Dashboard tabs (current / planned):

| Tab | Status | Content |
|-----|--------|---------|
| Projets | Done | Project list, AI audit pipeline (template → audit → validation), project bootstrap (create from scratch), MCP overview, per-project workflows/skills/doc viewer |
| Discussions | Done | Single/multi-agent chat, @mentions, orchestration, global discussions, archive/unarchive (swipe gestures), inline title editing, disabled agent detection. **⏹ Stop agent** button (CancellationToken via `AppState.cancel_registry`, CancelGuard RAII). **Partial response recovery (0.3.5)**: agent output checkpointed every ~30s/~100 chunks into `discussions.partial_response` (+ `partial_response_started_at` for chronological order). Backend restart converts dangling partials into Agent messages with "⚠️ Réflexion interrompue" footer + broadcasts `WsMessage::PartialResponseRecovered`. `POST /api/discussions/:id/dismiss-partial` for manual recovery. `send_message` refuses a new run while a partial is pending (`partial_pending` SSE error → frontend waits or dismisses). **Structured agent questions (0.3.5)**: `{{var}}: question` patterns in agent messages auto-render a mini-form (`AgentQuestionForm`) above ChatInput. **0.5.0 — Test mode (worktree swap-in-main)**: `🧪 Tester cette version` CTA in the ChatHeader swaps the main repo to the discussion's branch, global banner stays pinned while active, single-click exit restores previous branch + pops auto-stash + re-creates the worktree. Triple preflight (worktree dirty, main dirty, detached HEAD) with a dedicated modal for the MainDirty case (stash-and-proceed / commit-first / cancel). `POST /api/discussions/:id/test-mode/{enter,exit}` return tagged envelopes. Persistent across reboots via migration 034. **0.5.0 — Decoder-loop detection**: agent streams now kill the child after 50 consecutive identical non-whitespace deltas (fixes Claude Opus extended-thinking `</thinking>`-loop leaking 76 KB into one response on EW-7189). Parser-level strip of literal `<thinking>` tags is the first line of defense. **0.5.0 — Prompt over stdin for Claude Code**: `start_agent_with_config` writes the prompt to `stdin` instead of argv, bypassing Linux `ARG_MAX` (~128 KiB). `--append-system-prompt` still travels via argv but truncates at 100 KiB with a clear marker. **Split into 8 components**: DiscussionsPage (orchestrator) + ChatHeader, ChatInput, DiscussionSidebar, NewDiscussionForm, MessageBubble, SwipeableDiscItem, AgentQuestionForm — plus 2 test-mode components (`TestModeBanner`, `TestModeModal`) in 0.5.0 |
| Plugins | Done | Plugin registry — card grid + category pills, inline detail panel, per-project navigation. **0.5.0 — plugin kind: MCP \| API \| hybrid** (see §12). **56 plugins: 53 MCPs + 3 API plugins** (Chartbeat `apikey` query, Adobe Analytics OAuth2 S2S, Google Search `apikey` query). Per-card badges `🔌 MCP` / `🌐 API` / `MCP + API`. Kind-filter pills `All \| MCP \| API` on top. Publisher origin badges (official/community). Per-project MCP load indicator (green/orange/red). Env placeholders with realistic hints (sourced from `api_spec.config_keys` for API plugins, else static map). Eye toggle on add form; API plugins auto-expose non-secret fields as plain text with inline description. OAuth2 plugins get Kronn-managed bearer refresh transparent to the agent. Recent additions: MongoDB, Kubernetes, Qdrant, Perplexity, Microsoft 365, Chartbeat, Adobe Analytics, Google Search. Puppeteer removed (use Playwright). |
| Automatisation | Done | Two tabs: **Workflows** + **Quick Prompts**. Workflows: list (grouped by project), creation wizard (simple 3-step + advanced 5-step), detail + runs with live SSE, manual trigger, run deletion. MCP-based suggestions (10 templates). Structured inter-step contracts. AI Architect ("Create with AI" → discussion → `KRONN:WORKFLOW_READY`). Test step (dry-run + live streaming, state survives tab switches via module-level tracker). Starter templates (6 examples). Raw cron editor. **⏹ Cancel run** with cascade to child batch discussions via `parent_run_id`. **Notify step (0.3.5)**: `StepType::Notify` with webhook support (POST/PUT/GET), zero tokens, template rendering in URL + body. **Quick Prompts**: reusable prompt templates with `{{variables}}` and conditional sections `{{#var}}text{{/var}}`. Launch creates a discussion with rendered prompt and dynamic title. **Batch Quick Prompts (0.3.5)**: fan-out to N items (tickets / list / resolved template), each child gets its own discussion + optional worktree, aggregated in sidebar groups. Dry-run preview with per-item rendered prompt + per-item test button. |
| Config | Done | Multi-key API management (incl. Mistral/Vibe API keys), token usage tracking, language, agent detection + permissions, agent usage dashboard links, Directives CRUD with live cards, DB management (**export ZIP** with data.json + config.toml, **import ZIP/JSON** with config merge + path remapping). **Global context (0.3.5)**: markdown textarea + mode dropdown (always/no_project/never), injected into agent prompts via `ServerConfig.global_context`. Skills/Profiles are now managed per-project on the Project page. **Skill auto-trigger opt-out (0.5.1)**: per-skill toggle backed by `auto_triggers` table — disable a skill from contributing to prompt injection without removing it. |

Note: the old "Agents" tab has been merged into Config. Nav order: Projets → Discussions → Plugins → Workflows → Config. **"?" button** in nav replays the guided tour.

**Guided tour (0.3.6)**: 17-step interactive onboarding auto-launched on first visit. 5 acts (Projets → Plugins → Discussions → Automatisation → Config). 4 interactive steps with `waitForClick` (user must click the real UI element — pulse animation, "Next" blocked). Spotlight via box-shadow cutout, tooltip auto-positioned. Ends on Discussions page. Persistence: `kronn:tour-completed` in localStorage. Components: `TourProvider` (context + state machine), `TourOverlay` (portal), `tourSteps.ts` (declarative step definitions), `useTourPositioning.ts` (placement + MutationObserver).

### Project Bootstrap (create from scratch)

`POST /api/projects/bootstrap` — creates a new project directory, initializes git, installs AI template, creates a bootstrap discussion with architect + product-owner + entrepreneur profiles (3 profiles). **Bootstrap++**: skill `bootstrap-architect` auto-injected for gated validation flow:
1. Agent reads uploaded context files (architecture docs, specs, PRDs) → produces architecture summary → `KRONN:ARCHITECTURE_READY` → CTA validates
2. Agent generates project plan (epics, stories, estimates) → `KRONN:PLAN_READY` → CTA validates
3. Agent creates issues on tracker via MCP → `KRONN:ISSUES_CREATED` → CTA navigates to project

Frontend modal includes **drag & drop file upload** for documents. Files uploaded as context files after discussion creation. `BootstrapProjectRequest` accepts `skill_ids` for skill injection.

### Pre-audit briefing (optional)

`POST /api/projects/:id/start-briefing` — creates a briefing discussion where the AI asks 5 quick questions (project purpose, stack, team, conventions, watch points). The agent writes `ai/briefing.md` and emits `KRONN:BRIEFING_COMPLETE`. The briefing content is injected into each audit step via `PROMPT_PREAMBLE`. Agents without filesystem access (Vibe) are excluded from briefing/audit.

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
- When the AI finishes all questions, it includes `KRONN:VALIDATION_COMPLETE` in its last message. This triggers a green banner in the discussion with a "Marquer l'audit comme valide" button. Similarly, `KRONN:BRIEFING_COMPLETE` signals the end of a pre-audit briefing discussion. `KRONN:WORKFLOW_READY` signals the AI Architect has produced a deployable workflow JSON (extracted from ```json block → one-click creation). **Bootstrap++ signals**: `KRONN:ARCHITECTURE_READY` → validate architecture, `KRONN:PLAN_READY` → validate plan, `KRONN:ISSUES_CREATED` → view project. Each gate sends a user message to continue the agent.
- **Mark as validated**: injects `<!-- KRONN:VALIDATED:date -->` marker into `ai/index.md`.
- AI config file badges (CLAUDE.md, .cursorrules, etc.) shown on a second line below the status badges.

### Audit drift detection

`GET /api/projects/:id/drift` — compares source file checksums against `ai/checksums.json` (generated during audit). Returns stale sections without consuming tokens. `POST /api/projects/:id/partial-audit` re-runs only stale steps (~3-5K tokens vs ~20K for full audit). UI shows an amber badge on stale projects with a "Mettre à jour" button.

**MCP drift auto-detection**: adding/removing/relinking a plugin on an audited project automatically invalidates the `.mcp.json` checksum, flagging drift for step 8 (MCP introspection) re-run.

### Workflow suggestions

`GET /api/projects/:id/workflow-suggestions` — matches installed MCPs against a hardcoded catalogue of 10 workflow templates. Returns suggestions with multi-step prompts, pre-filled triggers, and audience tags (dev/pm/ops). Suggestions use structured inter-step contracts for reliable data passing between collection and synthesis steps.

### Structured inter-step contract

Workflow steps can declare `output_format: Structured` (default: `FreeText`). When structured:
1. Engine auto-injects `---STEP_OUTPUT---` envelope instructions into the prompt
2. After execution, extracts JSON envelope `{"data": ..., "status": "OK|NO_RESULTS|ERROR", "summary": "..."}`
3. If extraction fails, sends a repair prompt (truncated to 2000 chars) for reformatting
4. Downstream steps access `{{previous_step.data}}`, `{{previous_step.summary}}`, `{{previous_step.status}}`
5. `status: "NO_RESULTS"` is detected by the condition system (replaces `[SIGNAL: NO_RESULTS]` for structured steps)

### Desktop app (Tauri)

- **System tray**: closing the window hides to tray, backend + scheduler keep running. Tray menu: "Ouvrir Kronn" / "Quitter". Double-click to reopen.
- **Wake lock**: when cron workflows are active, prevents OS sleep (Windows: `SetThreadExecutionState`, macOS: `caffeinate -w`). Auto-releases when no cron workflows remain.
- **PATH enrichment**: GUI apps on macOS inherit minimal PATH. `enrich_path()` at startup adds homebrew, npm global, cargo, nvm, fnm, bun, uv directories if they exist.
- **Ad-hoc codesigning**: macOS builds are signed with `codesign --force --deep -s -` when no Apple Developer certificate is configured. Users may need `xattr -cr /Applications/Kronn.app`.
- **Agent detection**: uses native PATH (enriched) + npx probe fallback. Agents found via npx only show as "npx" (orange badge) not "installed" (green badge).

### Plugin kind: MCP | API | hybrid (0.5.0)

Kronn plugins expose capabilities to agents in two ways:

| Kind | `mcp_servers.transport` | `mcp_servers.api_spec_json` | How agents use it |
|------|-------------------------|----------------------------|-------------------|
| **MCP** | `Stdio \| Sse \| Streamable` | NULL | Synced to `.mcp.json` / Vibe / Kiro / Gemini configs. Agents discover tools via `mcp__<server>__<tool>` naming. |
| **API** | `ApiOnly` | `{...}` | Skipped in `.mcp.json`. Capability surfaces via prompt injection (`## REST APIs available` section with curl examples + auth). Agents call via Bash `curl`. |
| **Hybrid** | any MCP variant | `{...}` | Both of the above. Agent picks the right approach. (e.g. Jira has both an MCP server and a REST API.) |

**Plumbing**:
- Rust models in `backend/src/models/mod.rs`: `McpTransport::ApiOnly`, `ApiSpec { base_url, auth, endpoints, docs_url, config_keys }`, `ApiAuthKind::{ApiKeyQuery, ApiKeyHeader, Bearer, OAuth2ClientCredentials, None}`, `ApiEndpoint`, `ApiConfigKey`, `OAuth2ExtraHeader`.
- Migration 035 adds `api_spec_json` (nullable) to `mcp_servers`. Zero impact on existing rows.
- `build_api_context_block()` in `core/mcp_scanner.rs` emits the API section from `(server, decrypted_env)` pairs. Called from `make_agent_stream` only for project discussions with at least one active API plugin. Disk MCP context is concatenated when both are present.
- `sync_project_mcps_to_disk()` matches on `transport` — `ApiOnly` is a silent skip.
- The `collect_active_api_plugins()` helper fetches + decrypts active API configs per project.
- `config_keys` lets a plugin declare non-secret parameters (e.g. Chartbeat's `host`, Adobe's `company_id`). The UI renders them as plain inputs with the provided `label` + `placeholder` + `description`; the prompt injection surfaces them alongside the auth so the agent has enough to build a full URL.
- `{ENV_KEY}` templating works in `ApiSpec.base_url` AND `OAuth2ExtraHeader.value_template`. Missing keys render as `<NOT_CONFIGURED:KEY>`.
- Default context (`default_context` on the registry entry) is auto-written to `ai/operations/mcp-servers/<slug>.md` at install time — for API plugins too.

**OAuth2 client-credentials (`ApiAuthKind::OAuth2ClientCredentials`)**:
- New module `backend/src/core/oauth2_cache.rs` — in-memory `HashMap<config_id, CachedToken>` on `AppState.oauth2_cache` (Tokio `Mutex`, `tokio::sync::Mutex`). Thread-safe refresh: concurrent discussion starts on the same plugin share one HTTPS exchange.
- Exchange flow: `POST <token_url>` with `grant_type=client_credentials&client_id=…&client_secret=…&scope=…` (form-urlencoded). Parses `access_token` + `expires_in` from the JSON response. `refresh_at = now + expires_in - 30s` safety margin.
- Async resolver in `make_agent_stream` runs BEFORE `build_api_context_block`: for every plugin with `OAuth2ClientCredentials`, calls `resolve_token()` and injects the result into the plugin's env map under virtual keys `__access_token__` (success) or `__token_error__` (failure). The sync context builder reads those without knowing the auth flow.
- Error-transparency: on token-exchange failure the context block shows *"TOKEN UNAVAILABLE — <reason>"* so the agent stops rather than firing unauthenticated requests.
- On backend restart the cache is empty; one HTTPS round-trip per active OAuth2 plugin on first use, no user-visible impact.

**Plugins shipped**:
- `api-chartbeat` — `https://api.chartbeat.com`, `apikey` query param. 21 endpoints: Live (sync GETs) + Historical (async `submit` → `status` → `fetch`, with `X-CB-AK` header). `CHARTBEAT_HOST` is a `config_key`.
- `api-adobe-analytics` — `https://analytics.adobe.io/api/{ADOBE_COMPANY_ID}` (path interpolation). OAuth2 client-credentials against Adobe IMS `/ims/token/v3`. 7 endpoints: `POST /reports`, `POST /reports/realtime`, `GET /dimensions`, `GET /metrics`, `GET /segments`, `GET /calculatedmetrics`, `GET /users/me`. Required extra headers via `OAuth2ExtraHeader.value_template`: `x-api-key: {ADOBE_CLIENT_ID}` + `x-proxy-global-company-id: {ADOBE_COMPANY_ID}`. Config keys: `ADOBE_COMPANY_ID`, `ADOBE_ORG_ID`, `ADOBE_RSID` (non-secret).
- `api-google-search` — `https://www.googleapis.com/customsearch/v1`, `apikey=` query auth. One endpoint, rich param matrix (`q`, `num`, `start`, `dateRestrict`, `siteSearch`, `searchType`, `lr`, `gl`). `GOOGLE_SEARCH_CX` exposed as config_key — duplicate the plugin per Programmable Search Engine (site-scoped vs whole-web). 100 queries/day free; default_context documents quota + SEO use-cases (rank check, 7-day news, site search).

**Roadmap**:
- New workflow step type `ApiCall { plugin_id, endpoint, params }` that hits the API directly from the workflow engine — zero agent tokens. This is the "désagentification" vision (`ai/decisions.md` / `project_deagentify_workflows.md` memory). Plugin-kind abstraction is the enabler.
- Next OAuth2 plugins candidates: Google Analytics 4 Data API, Salesforce REST — same `OAuth2ClientCredentials` variant, different `token_url` + scopes + extra headers.

### Document generation — Kronn Docs (0.5.1)

Agents produce 5 file formats (PDF / DOCX / XLSX / CSV / PPTX) without the user installing anything.

**Sidecar architecture** (`backend/sidecars/docs/`): Python FastAPI + uvicorn on a random loopback port, started by backend during boot. Deterministic startup via `KRONN_DOCS_READY <port>` stdout marker. Dependencies: WeasyPrint (PDF), python-docx + BeautifulSoup (DOCX, HTML→Word mapping), XlsxWriter (XLSX), stdlib `csv` (CSV), python-pptx (PPTX). Setup is opt-in via `make docs-setup` — missing venv degrades to a clear "Document sidecar unavailable" error instead of a hard failure.

**Rust proxy** (`backend/src/api/docs.rs` + `backend/src/core/docs_sidecar.rs`): 5 endpoints `POST /api/docs/{pdf,docx,xlsx,csv,pptx}` + `GET /api/docs/file/:discussion_id/:filename`. All five POST handlers funnel through a single `proxy_to_sidecar()` helper — adding a format = one arm. Output files land in `~/.kronn/generated/<discussion_id>/`. Filename sanitization (alphanumerics + `-_ ` only, UUID suffix, extension forced) + canonicalize check in `download_file` guard against path traversal.

**Agent contract** — skill `kronn-docs.md` ships two fence conventions:
- ```` ```kronn-doc-preview ```` — HTML body used for PDF + DOCX. Frontend renders a sandboxed iframe (`sandbox=""`) + two export buttons.
- ```` ```kronn-doc-data ```` — JSON payload `{format, ...}` for structured formats (XLSX / CSV / PPTX). No preview; compact card with summary (row count, sheet count, slide count) + single export button.

Auto-activation: the skill carries `auto_triggers.common/fr/en/es` regex buckets — "génère un rapport PDF", "create a presentation", "exporta hoja xlsx" etc. Matched skills auto-inject into the system prompt. Per-skill opt-out via Settings (`auto_triggers` table).

**Frontend** — `frontend/src/components/DocPreview.tsx` (HTML formats) + `DocDataExport.tsx` (structured). Both detected in `MarkdownContent` (`MessageBubble.tsx`) by the fence's `language-kronn-doc-*` class. Malformed JSON / unknown format discriminator falls back to a normal `<pre>` so a bad agent message can't blow up the chat.

---

## 10. Multi-agent configuration

Redirectors to this file: `CLAUDE.md`, `GEMINI.md`, `AGENTS.md`, `.kiro/steering/instructions.md`, `.vibe/instructions.md`, `.cursorrules`, `.cursor/rules/repo-instructions.mdc`, `.github/copilot-instructions.md`, `.windsurfrules`, `.clinerules`.

**Maintenance rule**: all content lives in `ai/`. Redirectors contain a summary of critical rules + pointer to `ai/index.md` as source of truth.

---

## 11. Last updated

AI context last reviewed: **2026-04-22** (v0.5.1 released — Kronn Docs: Python sidecar + 5 format endpoints + skill auto-triggers + per-skill opt-out + DocPreview / DocDataExport fences. Light theme expert rework + secret unlock system + 3 hidden themes + Batman profile).

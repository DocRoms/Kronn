# Architecture (AI context)

> Folder structure: `ai/repo-map.md`.

## Apps / services (facts)

Three Docker services behind nginx gateway:

| Service | Port | Role |
|---------|------|------|
| `backend` | 3001 | Rust/axum API server |
| `frontend` | 5173 | Vite dev server (React) |
| `gateway` | 3456 | nginx reverse proxy (routes `/api/*` to backend, rest to frontend) |

## Key patterns (facts)

### API pattern
- All endpoints return `ApiResponse<T>` wrapper: `{ success: bool, data: T|null, error: string|null }`.
- Routes registered in `backend/src/main.rs`, handlers in `backend/src/api/<domain>.rs`.
- Axum 0.7 method chaining: same path, multiple methods → `.route("/path", get(h1).post(h2))`.

### SSE streaming
- Agent responses stream via Server-Sent Events (not WebSocket).
- Events: `chunk` (text delta — raw concatenation, no separator), `done`, `error`.
- Orchestration adds: `system`, `round`, `agent_start`, `agent_done`.
- Workflow runs use SSE for real-time progress: `RunEvent` enum with `StepStart`, `StepDone`, `RunDone`, `RunError`. Frontend shows a live progress panel with step indicators (pulse animation for current, check/X for completed).
- Frontend uses `ReadableStream` reader + manual SSE parsing in `api.ts:_streamSSE()`.
- `AbortController` + `signal` for cancellation. `finished` boolean guard prevents double `onDone`.

### Agent execution
- `agents/runner.rs` spawns CLI processes (`claude`, `codex`, `vibe`, `gemini`) with `--print` / non-interactive flags.
- **Two output modes**: `Text` (line-by-line stdout, default for Codex/Vibe/Gemini) and `StreamJson` (Claude Code with `--output-format stream-json --verbose --include-partial-messages`). In StreamJson mode, each line is a JSON event parsed by `parse_claude_stream_line()` — text deltas from `stream_event` events, token usage from `result` event.
- Agents run in the project's directory context (or temp dir for global discussions).
- **Runtime probe**: if no local binary is found, `probe_runtime()` tests npx availability (15s timeout, 5min cache). `AgentDetection.runtime_available` distinguishes "installed locally" from "runnable via npx". Frontend uses `isUsable(agent) = (installed || runtime_available) && enabled`.
- MCPs work with all 3 agents: Claude Code (`.mcp.json`), Codex (`~/.codex/config.toml`), Vibe (`.vibe/config.toml`). Disk sync writes all formats simultaneously.
- `AgentConfig` has `full_access: bool` field (persisted in config.toml). When enabled, runner adds `--dangerously-skip-permissions` (Claude) or `--full-auto` (Codex).
- API: `GET/POST /api/config/agent-access` to read/set the full_access flag. UI toggle in Config > Agents card.
- **Agent lifecycle**: agents can be uninstalled (`POST /api/agents/uninstall`) or toggled on/off (`POST /api/agents/toggle`). Disabled agents tracked in `AppConfig.disabled_agents: Vec<AgentType>`. `AgentDetection` includes `enabled: bool`. Uninstall uses platform-specific commands (npm for Claude/Codex, uv/pipx/pip3 for Vibe).
- **Known issue — file permissions**: agents run as `root` inside the Docker container. Files created/modified on host-mounted volumes (`:rw`) end up owned by `root:root` on the host. Fix: pass host UID/GID via `KRONN_UID`/`KRONN_GID` env vars in docker-compose, then run agent processes with `--user $KRONN_UID:$KRONN_GID` or use `tokio::process::Command` with `.uid()/.gid()`.
- **Path resolution**: `resolve_host_path` uses Docker mount priority (prefers /host-home over /home/priol).

### Discussions
- `Discussion.project_id` is `Option<String>` (Rust) / `string | null` (TS).
- Discussions without a project are "global" — shown under "Général" group in the sidebar.
- Agent runs in a temp directory for global discussions (no project context).
- `CreateDiscussionRequest.project_id` is also optional; frontend offers "Aucun projet" option.

### State management
- Backend: `AppState` holds `db: Arc<Database>` (SQLite) and `config: Arc<RwLock<AppConfig>>`.
- `Database` struct wraps `Mutex<Connection>` with `with_conn()` async accessor.
- Data persisted in `kronn.db` with WAL mode and foreign keys enabled.
- Migrations run via `backend/src/db/migrations.rs` (versioned SQL files, executed before Mutex wrap to avoid blocking_lock panic).
- Frontend: `useApi` hook for data fetching. Dashboard.tsx is the main shell; sub-pages (McpPage.tsx, WorkflowsPage.tsx) receive data as props. UI state managed locally with `useState`.
- `useMemo` for computed values (agent mentions filtering, unread counts). Conditional polling (only active tab).
- `ErrorBoundary` class component wraps lazy-loaded routes. `React.lazy` + `Suspense` for code splitting (SetupWizard, Dashboard).
- `AbortController` cleanup on component unmount for SSE streams.
- Shared constants extracted to `lib/constants.ts` (AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES).
- Unread badges persisted in localStorage.
- No global state library (no Redux, Zustand, etc.).

### Type bridge
- Rust models annotated with `#[derive(TS)]` from `ts-rs` crate.
- `make typegen` generates `frontend/src/types/generated.ts`.
- Rust is source of truth; TypeScript follows.

### i18n (internationalization)
- Lightweight custom translation system (no external lib).
- 3 UI locales: `fr`, `en`, `es` — defined in `frontend/src/lib/i18n.ts`.
- UI language stored in `localStorage` (`kronn:ui-locale`), separate from agent output language (backend config).
- `I18nContext.tsx` provides `useT()` hook returning `t(key, ...args)` for components.
- Translation keys use dot notation: `nav.projects`, `projects.search`, `config.tokens.title`, etc.
- String interpolation with `{0}`, `{1}` positional placeholders.
- Default locale: `fr`.

### Multi-agent orchestration
- Discussion can involve multiple agents debating in configurable rounds (1–3, default 2 in UI).
- Primary agent (discussion owner) speaks last each round and produces final synthesis.
- Anti-repetition prompts keep rounds concise.
- Language configurable globally (fr/en/zh/br), injected into all agent prompts.

### Workflows (implemented — replaces scheduled tasks)

Unified automation system: `Trigger → Steps`. Superset of OpenAI Symphony's WORKFLOW.md format. Post-step operations (create PR, comment on issue, etc.) are handled directly by agents using MCP tools within steps, not as a separate "actions" phase.

**Triggers:**
- **Cron** — scheduled execution (simple recurring task). 1 tick = 1 run.
- **Tracker** — polls an issue tracker API (GitHub, Linear...) at a cron interval. Each new matching issue = 1 separate run with issue context injected via Liquid templates. Pull-based (polling), not push (webhooks). Tracks processed issue IDs for reconciliation (no duplicate runs).
- **Manual** — triggered from dashboard UI or CLI.

**Steps:**
- Sequential execution, each step runs an agent with optional per-step MCPs (resolved and synced before execution).
- Steps can use `mode: debate` for multi-agent discussion at any point.
- Context flows between steps via Liquid-compatible template variables: `{{issue.title}}`, `{{issue.body}}`, `{{issue.number}}`, `{{issue.url}}`, `{{issue.labels}}`, `{{previous_step.output}}`, `{{steps.<name>.output}}`.
- **Conditional branching**: `on_result` rules per step — e.g. `{ contains: "NO_RESULTS", action: stop }`. Actions: `Stop`, `Skip`, `Goto(step_name)`.
- **Per-step agent config**: optional `AgentSettings { model, reasoning_effort, max_tokens }` override.
- **Stall detection**: configurable timeout — kill step if no agent output for N seconds.
- **Retry**: exponential backoff for failed steps (`max_retries`, `backoff: exponential`).

**Workspace:**
- Isolated git worktree per run (`git worktree add`), branch: `kronn/<workflow>/<run-id>`.
- Lifecycle hooks (shell commands): `after_create`, `before_run`, `after_run`, `before_remove`.
- Cleanup on completion/failure.

**MCP injection:**
- `read_all_mcp_contexts()` reads `.mcp.json` and per-project MCP context files (`ai/operations/mcp-servers/*.md`).
- Available MCP servers are listed in agent prompts with instruction to use `mcp__<server>__<tool>` tools instead of Bash workarounds.
- Applied to both workflow steps and discussions.

**Safety:**
- Sandbox mode (Docker), max files/lines changed, approval gates.
- Concurrency limit per workflow (max simultaneous runs).

**Token accounting:**
- Per-step and per-run token totals tracked in `StepResult` and `WorkflowRun`.

**Key design decisions:**
- Workflows are created step-by-step via the dashboard UI (wizard), not just WORKFLOW.md files.
- WORKFLOW.md files can be imported/detected from repos (Symphony format → single-step Kronn workflow).
- Import auto-detects missing MCPs and suggests installation from registry.
- Storage format in DB is JSON (not YAML).
- Reuses existing agent runner and multi-agent debate system.
- Symphony is a strict subset: single-agent, single-prompt, tracker-driven. Kronn adds multi-step, multi-agent, conditional branching, per-step MCPs.

### MCP system (3-tier architecture)

Kronn manages MCPs with a 3-tier model:

```
mcp_servers (type)  →  mcp_configs (configured instance)  →  mcp_config_projects (N:N linkage)
```

**Servers** represent an MCP type (e.g. "GitHub"). Can come from the built-in registry (19 official servers), be detected from `.mcp.json` files, or be added manually.

**Configs** are configured instances of a server with encrypted env vars (AES-256-GCM), a label, and optional args override. One server can have multiple configs (e.g. two GitHub configs with different tokens). Deduplication via FNV-1a hash of (transport + args + env values).

**Project linkage** — N:N relationship. A config can be linked to multiple projects. The `is_global` flag means "applies to all projects" without explicit per-project linkage.

**Registry** — 19 built-in official MCP servers in `core/registry.rs`, grouped by category: Git & Code, Databases, Cloud & Infra, Search & Web, Monitoring, Communication, Project Management, Utilities. Each has `env_keys` listing required environment variables, plus optional `token_url` (link to provider's token generation page) and `token_help` (short guidance text).

**Disk sync (3 formats)** — When linkages or config values change, Kronn writes agent-specific config files:

| Agent | Config file | Scope | Format |
|-------|------------|-------|--------|
| Claude Code | `.mcp.json` (in project dir) | Per-project | JSON (`mcpServers` object) |
| Vibe | `.vibe/config.toml` (in project dir) | Per-project | TOML (`[[mcp_servers]]` array) |
| Codex | `~/.codex/config.toml` (global) | Global (all MCPs) | TOML (`[mcp_servers.<name>]` tables) |

Sync triggers: toggle project, toggle global, create/update/delete config. Key naming: single config for a server → `server.name.to_lowercase()`, multiple configs of same server → `config.label`. Files are added to `.gitignore`. Codex only supports stdio transport (SSE/streamable MCPs are skipped). Codex global config preserves non-MCP settings (model, approval_policy, etc.).

**MCP context files** — Per-MCP per-project instruction files at `ai/operations/mcp-servers/<slug>.md`. Auto-created with a default template on first disk sync. Customized files are injected into agent system prompts via `--append-system-prompt`. The `McpOverview` response includes `customized_contexts` (list of `"slug:projectId"` pairs) so the frontend can show colored icons for customized vs default context files.

**Detection & matching** — `POST /api/mcps/refresh` scans all projects' `.mcp.json` files, matches detected entries against the registry by command + package name (with version stripping), migrates `detected:*` server IDs to registry IDs, and cleans up orphan servers.

**Secret editing** — Inline editing of encrypted env vars directly in the MCP page. Per-field visibility toggle (eye icon) to show/hide individual values. On save, secrets are re-encrypted, config hash recomputed, and `.mcp.json` re-synced to all linked projects.

**API endpoints:**
- `GET /api/mcps` — overview (servers + configs with masked secrets + customized_contexts)
- `GET /api/mcps/registry` — built-in registry (searchable, includes token_url/token_help)
- `POST /api/mcps/refresh` — scan & detect
- `POST /api/mcps/configs` — create config (auto-creates server from registry if needed)
- `PUT /api/mcps/configs/:id` — update config (label, env, global, args)
- `DELETE /api/mcps/configs/:id` — delete config
- `PUT /api/mcps/configs/:id/projects` — set project linkages
- `POST /api/mcps/configs/:id/reveal` — decrypt and reveal secrets
- `GET /api/mcps/context/:project_id` — list MCP context files for a project
- `GET /api/mcps/context/:project_id/:slug` — read a single MCP context file
- `PUT /api/mcps/context/:project_id/:slug` — update a MCP context file

### AI audit pipeline

4-state system computed from filesystem (not DB):

```
NoTemplate → TemplateInstalled → Audited → Validated
```

- **Detection**: `scanner::detect_audit_status()` checks `ai/index.md` existence, `KRONN:BOOTSTRAP`/`{{` markers, and `KRONN:VALIDATED` marker.
- **TODO counting**: `scanner::count_ai_todos()` walks `ai/*.md` files and counts `<!-- TODO` occurrences. Exposed as `Project.ai_todo_count` (computed on-the-fly by `enrich_audit_status()`).
- **Template install** (`POST /api/projects/:id/install-template`): copies `ai/` skeleton + redirectors (CLAUDE.md, .cursorrules, etc.) non-destructively, injects bootstrap prompt block (`KRONN:BOOTSTRAP:START` to `KRONN:BOOTSTRAP:END`).
- **AI audit** (`POST /api/projects/:id/ai-audit`): SSE-streamed 10-step analysis. Each step runs an agent call with `full_access: true`. Bootstrap block removed before audit starts. Steps defined in `ANALYSIS_STEPS` constant.
- **Validation**: creates a Discussion with title "Validation audit AI" and a locked prompt. The AI asks questions about ambiguities/TODOs, updates `ai/` files after each answer. Frontend detects validation-in-progress by matching discussion title + project_id. Project page only shows "validation en cours" + link (no validate button).
- **Completion detection**: the prompt instructs the AI to include `KRONN:VALIDATION_COMPLETE` in its final message. Frontend detects this in the last agent message and shows a green banner with "Marquer l'audit comme valide" button — only in the discussion view.
- **Mark validated** (`POST /api/projects/:id/validate-audit`): injects `<!-- KRONN:VALIDATED:YYYY-MM-DD -->` at end of `ai/index.md`.

**API endpoints:**
- `POST /api/projects/:id/install-template` — copy template, inject bootstrap
- `POST /api/projects/:id/ai-audit` — SSE streaming 10-step audit
- `POST /api/projects/:id/validate-audit` — mark audit as validated

### DB management API
- `GET /api/config/db-info` — returns DB size and record counts per table.
- `GET /api/config/export` — full JSON dump of all data.
- `POST /api/config/import` — restore from JSON dump.
- UI in Config page with counters, export button, import button.

### Agent colors (consistent everywhere)
- Claude Code: `#D4714E` (terracotta)
- Codex: `#10a37f` (OpenAI green)
- Vibe: `#FF7000` (Mistral orange)
- Gemini CLI: `#4285f4` (Google blue)

## Separation of concerns

- `models/` — Pure data structures, no logic.
- `api/` — HTTP handlers, request validation, SSE streaming.
- `core/` — Business logic (config, scanning, registry).
- `db/` — SQLite persistence, migrations, CRUD operations.
- `agents/` — External CLI process management.
- `workflows/` — Workflow engine: triggers, steps, template rendering, workspaces, tracker adapters (GitHub).
- `scheduler/` — Legacy cron-based task execution (to be replaced by workflows).

## Data flow

```
User → nginx (gateway:3456)
  → /api/* → backend (axum:3001)
    → handlers → state (SQLite via Database struct) / agent runner → SSE response
  → /* → frontend (vite:5173)
    → React SPA → fetch /api/* via api.ts
```

### Workflow execution flow (implemented)

```
WorkflowEngine (polling loop, ticks every 30s)
  → check cron triggers → spawn run (respect concurrency_limit)
  → check tracker triggers → poll API → reconcile (skip already-processed) → spawn run per new issue
  → manual trigger via API → spawn run immediately

WorkflowRunner (per run)
  → create workspace (git worktree add -b kronn/<workflow>/<run-id>)
  → run workspace hooks: after_create
  → run workspace hooks: before_run
  → for each step:
    → resolve per-step MCPs → sync to disk
    → render prompt template (Liquid: issue context, previous_step.output, steps.<name>.output)
    → if mode=normal: call agent runner (with optional AgentSettings override)
    → if mode=debate: call multi-agent orchestration
    → monitor stall timeout (kill if no output for N seconds)
    → on failure: retry with exponential backoff (if configured)
    → evaluate on_result conditions (stop/skip/goto)
    → record StepResult (output, tokens, duration)
  → run workspace hooks: after_run
  → cleanup workspace (git worktree remove)
  → run workspace hooks: before_remove
  → emit SSE events throughout for real-time UI updates
```

# Glossary — Project terminology

Project-specific terms. For deep dives, follow the linked `ai/architecture/` files.

---

## Architecture / stack

**AppState** — Axum shared state holding `db: Arc<Database>`, `config: Arc<RwLock<AppConfig>>`, `agent_semaphore: Arc<Semaphore>`, and `workflow_engine: Arc<WorkflowEngine>`. See `backend/src/lib.rs`.

**Gateway** — nginx reverse proxy (Docker service) routing `/api/*` to backend and `/*` to frontend. Port 3456.

**SSE (Server-Sent Events)** — Streaming protocol used for agent responses and workflow run updates. Events: `chunk`, `done`, `error`, `system`, `round`, `agent_start`, `agent_done`.

**Type bridge / typegen** — `ts-rs` crate auto-generates TypeScript types from Rust `#[derive(TS)]` models. Run `make typegen`.

**Database** — `Database` struct in `backend/src/db/mod.rs`. Wraps `Mutex<Connection>` for SQLite access. Uses `with_conn()` async accessor. Data persisted in `kronn.db` with WAL mode and foreign keys enabled.

**Migration** — Versioned SQL schema evolution in `backend/src/db/migrations.rs`. SQL files in `backend/src/db/sql/` (e.g., `001_initial.sql`, `002_mcp_redesign.sql`). Run before Mutex wrap to avoid async runtime issues.

**Encryption** — AES-256-GCM encryption for MCP secrets (env vars). Key derived from `encryption_secret` in config.toml via `core/crypto.rs`.

**DbInfo** — Response from `GET /api/config/db-info`: database file size and record counts per table.

**DbExport** — Full JSON dump of all database tables, retrieved via `GET /api/config/export` and restored via `POST /api/config/import`.

**ServerConfig** — Backend server configuration: `host`, `port`, `domain` (for CORS), `auth_token` (opt-in Bearer auth), `auth_enabled` (distinguishes user-set from auto-generated token), `max_concurrent_agents` (1–20, default 5).

**ServerConfigPublic** — Public view of ServerConfig returned by `GET /api/config/server`: `host`, `port`, `domain`, `max_concurrent_agents`, `auth_enabled`. Excludes `auth_token` for security.

**auth_enabled** — Boolean flag on `ServerConfig`. When `true`, the auth token was explicitly enabled by the user from the Settings UI. Prevents legacy auto-generated tokens from blocking access after upgrade.

**Agent semaphore** — `tokio::sync::Semaphore` in `AppState` limiting concurrent agent processes. Acquired with `acquire_owned()` in `make_agent_stream` and `orchestrate`. Size configurable via `max_concurrent_agents`.

**full_access** — Boolean field on `AgentConfig` (persisted in config.toml). When true, agent runner adds `--dangerously-skip-permissions` (Claude) or `--full-auto` (Codex) to CLI invocations. Controlled via `GET/POST /api/config/agent-access`.

## Domain concepts

**Project** — A registered git repository managed by Kronn. Has MCPs, workflows, and AI config detection.

**Discussion** — A chat conversation with one or more AI agents, optionally tied to a project (`project_id: Option<String>`). Supports single-agent and multi-agent (orchestration) modes. Global discussions (no project) appear under "Général" in the sidebar.

**Orchestration** — Multi-agent debate: multiple agents discuss in configurable rounds (1–3, default 2 in UI). Primary agent speaks last and synthesizes. Round count configurable from the debate popover.

**MCP (Model Context Protocol)** — Standardized protocol for giving AI agents access to tools/data. Kronn uses a 3-tier model: servers → configs → project linkages.

**McpServer** — A known MCP server type (e.g. "GitHub"). Has id, name, description, transport, and source (Registry, Detected, Manual). Stored in `mcp_servers` table.

**McpConfig** — A configured instance of an MCP server with encrypted env vars, label, and optional args override. One server can have multiple configs (e.g. two GitHub configs with different tokens). Stored in `mcp_configs` table.

**McpConfigDisplay** — Read-only projection of McpConfig with masked secrets, server name, and linked project names. Used in API responses.

**McpDefinition** — A template MCP from the built-in registry (name, transport, env_keys, tags, token_url, token_help). 26 official servers grouped by category (Git & Code, Databases, Cloud & Infra, Search & Web, Analytics & Monitoring, Communication, Project Management, Design, Knowledge & Docs, Payments, SEO, Files, Email). `token_url` links to the provider's token generation page; `token_help` provides a short description.

**McpInstance** — Legacy type kept for backward compatibility in the Project struct.

**Config hash** — FNV-1a hash of (transport + args + env values) used to deduplicate identical MCP configs.

**MCP disk sync** — When project-MCP linkages or config values change, Kronn writes agent-specific config files: `.mcp.json` (Claude Code, per-project), `.vibe/config.toml` (Vibe, per-project), `~/.codex/config.toml` (Codex, global). Ensures files are in `.gitignore`. Key naming: single config → `server.name.to_lowercase()`, multiple configs → `config.label`. Codex keys are slugified (`^[a-zA-Z0-9_-]+$`). Codex only gets stdio MCPs (SSE/streamable skipped). Codex global config preserves non-MCP settings.

**MCP context file** — Per-project instructions for AI agents using a specific MCP. Stored at `ai/operations/mcp-servers/<slug>.md`. Auto-created with default template on first sync; customized files are injected into agent system prompts. Managed via `McpPage.tsx` context editor modal.

**MCP injection** — `read_all_mcp_contexts()` in `core/mcp_scanner.rs` reads `.mcp.json` and MCP context files, then generates a prompt section listing all available MCP servers. Injected into agent prompts for workflow steps and discussions so agents use `mcp__<server>__<tool>` tools instead of Bash workarounds.

**customized_contexts** — `Vec<String>` of `"slug:projectId"` pairs in `McpOverview` where the context file has been customized (not default template). Used by frontend to color FileText icons.

**AgentType** — Enum: `ClaudeCode`, `Codex`, `Vibe`, `GeminiCli`, `Kiro`, `Custom`. Determines which CLI to spawn. `DeepSeek` and `OpenCode` planned. Kiro uses `--trust-all-tools` (mandatory in `--no-interactive` mode) and `--wrap never` for clean output.

**scan_depth** — Configurable depth for git repository scanning (2–10, default 4). Stored in `ScanConfig.scan_depth` in config.toml. Adjustable via UI slider in Config page. API: `GET/POST /api/config/scan-depth`.

**disabled_agents** — `Vec<AgentType>` in `AppConfig` (persisted in config.toml). Agents in this list are installed but inactive (toggled off). Controlled via `POST /api/agents/toggle`.

**UILocale** — Frontend UI language type: `'fr' | 'en' | 'es'`. Stored in `localStorage` under `kronn:ui-locale`. Default: `fr`. Separate from backend agent output language.

**useT()** — React hook from `I18nContext.tsx`. Returns `t(key, ...args)` function for translating UI strings using the current locale.

**DetectedRepo** — A git repository found by the scanner in configured scan paths.

**AiConfigStatus** — Detection of existing AI config files in a project (CLAUDE.md, .cursor/, .ai/, etc.).

**AiAuditStatus** — Enum: `NoTemplate`, `TemplateInstalled`, `Audited`, `Validated`. Computed from filesystem state (not stored in DB). Detected by `scanner::detect_audit_status()`.

**ai_todo_count** — Number of `<!-- TODO -->` markers remaining in `ai/*.md` files. Computed on-the-fly by `scanner::count_ai_todos()`, exposed on `Project` struct.

**Bootstrap prompt** — Block injected into `ai/index.md` between `KRONN:BOOTSTRAP:START` and `KRONN:BOOTSTRAP:END` markers. Instructs AI agents to analyze the repo and fill the `ai/` skeleton. Removed before running the automated audit.

**Project Bootstrap** — Feature to create a new project from scratch via `POST /api/projects/bootstrap`. Creates directory, runs `git init`, installs AI template, and creates a bootstrap discussion with architect + product-owner profiles. The bootstrap prompt guides through Vision → Architecture → Structure → MVP → Action Plan. Parent directory resolved from existing projects' common parent path or `KRONN_REPOS_DIR` env var.

**BootstrapProjectRequest** — `{ name: string, description: string, agent: AgentType }`. Name is sanitized to kebab-case for the directory name.

**BootstrapProjectResponse** — `{ project_id: string, discussion_id: string }`. Frontend uses `discussion_id` to auto-navigate and trigger the agent.

**find_common_parent** — Internal function in `projects.rs` that computes the common parent directory of all existing projects in DB. Used by bootstrap to determine where to create new project directories. E.g., `/home/user/Repos/A` + `/home/user/Repos/B` → `/home/user/Repos`.

**Skill auto-detection** — During AI audit (between Phase 2 and Phase 3), `detect_project_skills()` scans project filesystem for config files (Cargo.toml → rust, tsconfig.json → typescript, go.mod → go, etc.) and saves detected skills to DB. Covers languages (rust, typescript, python, go, php), domain (devops, database, security), and business (web-performance, seo).

**Validation discussion** — Discussion with title "Validation audit AI" created from the project page. Uses a locked (read-only) prompt. The AI asks questions about ambiguities, updates `ai/` files after each answer. Detected by matching `title === 'Validation audit AI'` + `project_id`.

**KRONN:VALIDATED marker** — HTML comment `<!-- KRONN:VALIDATED:YYYY-MM-DD -->` injected at the end of `ai/index.md` when audit is marked as validated.

## Workflows

**Workflow** — Unified automation unit: `Trigger → Steps`. Replaces the old scheduled tasks concept. Created via 5-step dashboard wizard (infos → trigger → steps → config → resume) or imported from WORKFLOW.md files. Post-step operations (create PR, comment issue, etc.) are handled by agents using MCP tools within steps.

**WorkflowTrigger** — What starts a workflow run. Three types:
- **Cron** — time-based schedule. 1 tick = 1 run, always same prompt.
- **Tracker** — polls an issue tracker API at intervals. Each new matching issue = 1 run with issue context injected. Pull-based (polling, not webhooks).
- **Manual** — triggered from dashboard or CLI on demand.

**WorkflowStep** — A single unit of work within a workflow. Has an agent, optional per-step MCPs, a prompt template (Liquid-compatible), optional debate mode, optional `on_result` conditions, and optional `AgentSettings` override.

**StepMode** — `Normal` (single agent) or `Debate` (multi-agent rounds).

**StepCondition / on_result** — Conditional branching after a step completes. Rules like `{ contains: "NO_RESULTS", action: Stop }`. Keywords are auto-injected into agent prompts. Actions: `Stop` (end workflow), `Skip` (skip next step), `Goto(step_name)`.

**WorkflowAction** — (Legacy/deprecated) Post-step operation type kept in the data model for backward compatibility but no longer exposed in the UI wizard. Actions like creating PRs or commenting on issues should be done via MCP tools within steps.

**WorkflowRun** — A single execution of a workflow. Tracks status, step results, tokens used, workspace path. Statuses: `Pending`, `Running`, `Success`, `Failed`, `Cancelled`, `WaitingApproval`. Runs can be deleted individually or in bulk.

**StepResult** — Output of a single step execution: status, output text, tokens used, duration. Output available to subsequent steps via `{{steps.<name>.output}}`.

**RunEvent** — SSE event enum for live workflow run progress. Variants: `StepStart { step_name, step_index }`, `StepDone { step_result }`, `RunDone { status }`, `RunError { message }`. Frontend uses these to display a live progress panel with animated step indicators.

**WorkflowSafety** — Guards: sandbox mode (Docker), max files/lines changed, approval gate, concurrency limit.

**AgentSettings** — Per-step agent configuration override: `model`, `reasoning_effort`, `max_tokens`. Allows different steps to use different agent configurations.

**WorkspaceConfig / WorkspaceHooks** — Lifecycle hooks for workflow workspaces. Shell commands executed at: `after_create`, `before_run`, `after_run`, `before_remove`. Symphony-compatible.

**TrackerSource** — Trait for issue tracker integrations. GitHub implemented first, Linear/GitLab/Jira planned as community PRs.

**Tracker reconciliation** — Mechanism to avoid duplicate workflow runs for the same issue. Tracks processed issue IDs per workflow.

**Stall detection** — Configurable timeout per step. If the agent produces no output for N seconds, the step is killed and marked as failed.

**WorkflowEngine** — Background service that ticks every 30s, checks triggers, spawns runs. Enforces concurrency limits. Holds active workflows in memory.

**Workspace** — Isolated git worktree created for a workflow run. Branch: `kronn/<workflow>/<run-id>`. Cleaned up after completion.

**Symphony** — OpenAI's `WORKFLOW.md`-based automation system. Single-agent, single-prompt, tracker-driven. Kronn reads Symphony format natively as a strict subset — a Symphony WORKFLOW.md maps to a single-step Kronn workflow. Kronn adds: multi-step, multi-agent, conditional branching, per-step MCPs.

**Liquid templates** — Template engine used in workflow prompts. Variables: `{{issue.title}}`, `{{issue.body}}`, `{{issue.number}}`, `{{previous_step.output}}`, `{{steps.<name>.output}}`. Compatible with Symphony's template syntax.

## Agents

**Claude Code** — Anthropic's CLI coding agent (`claude` command).

**Codex** — OpenAI's CLI coding agent (`codex` command).

**Vibe** — Mistral's CLI coding agent (`vibe` command via `uvx --from mistral-vibe`). Config: `.vibe/config.toml` per-project.

**Gemini CLI** — Google's CLI coding agent (`gemini` command via `npm install -g @google/gemini-cli`). Headless mode: `gemini -p "prompt"`. Full access: `--yolo`. API key env: `GEMINI_API_KEY`. Color: `#4285f4`.

**Kiro** — Amazon's CLI coding agent (`kiro-cli` command). Headless mode: `kiro-cli chat --no-interactive`. Full access: `--trust-all-tools`. Auth: AWS Builder ID (no API key needed). Color: `#7B61FF` (Kiro purple).

**DeepSeek** — Planned agent support (waiting for official CLI).

**OpenCode** — Planned agent support.

**Agent Runner** — `backend/src/agents/runner.rs` — spawns CLI processes and streams stdout. Two output modes: `Text` (line-by-line) and `StreamJson` (Claude Code stream-json with token tracking). Frontend concatenates chunks directly (no separator).

**OutputMode** — Enum in `runner.rs`: `Text` (Codex, Vibe, Gemini — line-by-line stdout) or `StreamJson` (Claude Code — `--output-format stream-json` with delta events). Determines how `parse_claude_stream_line()` extracts text and token usage.

**runtime_available** — Boolean on `AgentDetection`. True when the agent is runnable via npx/uvx fallback even without a local binary. Probed with a 15s timeout, cached for 5 minutes. Frontend helper: `isUsable(agent) = (installed || runtime_available) && enabled`.

## UI

**Dashboard** — Main UI shell (~750 lines, `Dashboard.tsx`) with tabs: Projets, Discussions, MCPs, Workflows, Config. Each tab delegates to a sub-page. Project cards have collapsible accordion sections (Discussions, Doc AI, MCPs, Workflows, Skills, AI Context) with smart defaults based on audit status. Bootstrap modal for creating new projects from scratch.

**SettingsPage** — Extracted settings page (~670 lines, `SettingsPage.tsx`): UI/output language, agents config, multi-key token management, usage stats, DB management.

**DiscussionsPage** — Extracted discussions page (~1420 lines, `DiscussionsPage.tsx`): sidebar, chat, streaming, debate, archive/unarchive (swipe gestures), inline title editing, disabled agent detection.

**SwipeableDiscItem** — Component in DiscussionsPage for swipe-to-archive (right, blue) / swipe-to-delete (left, red) gestures on sidebar discussion items. Uses pointer events with 80px threshold.

**Setup Wizard** — First-run flow (`SetupWizard.tsx`) for configuring scan paths, detecting agents, and API tokens.

**Config** — Unified config tab, delegates to `SettingsPage.tsx`. (Agents tab merged into Config.)

**@mention** — Chat feature to target a specific agent (e.g., `@claude`) with autocomplete.

**Discussion archive** — Discussions can be archived (`archived: bool`, default false). Archived discussions hidden from main sidebar, shown in collapsible "Archives" section. Backend: `PATCH /api/discussions/:id` with `UpdateDiscussionRequest`.

**Discussion title editing** — Inline rename via double-click or pencil icon in chat header. Saves via `PATCH /api/discussions/:id`.

**Toast notifications** — `useToast()` hook in `frontend/src/hooks/useToast.ts`. Returns `{ toast, ToastContainer }`. Types: `success` (green), `error` (red), `info` (blue). Auto-dismiss 4s, max 3 visible, slide-in animation. Replaces all `alert()` calls.

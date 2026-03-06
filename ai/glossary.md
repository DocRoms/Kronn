# Glossary — Project terminology

Project-specific terms. For deep dives, follow the linked `ai/architecture/` files.

---

## Architecture / stack

**AppState** — Axum shared state holding `db: Arc<Database>` (SQLite) and `config: Arc<RwLock<AppConfig>>`. See `backend/src/main.rs`.

**Gateway** — nginx reverse proxy (Docker service) routing `/api/*` to backend and `/*` to frontend. Port 3456.

**SSE (Server-Sent Events)** — Streaming protocol used for agent responses and workflow run updates. Events: `chunk`, `done`, `error`, `system`, `round`, `agent_start`, `agent_done`.

**Type bridge / typegen** — `ts-rs` crate auto-generates TypeScript types from Rust `#[derive(TS)]` models. Run `make typegen`.

**Database** — `Database` struct in `backend/src/db/mod.rs`. Wraps `Mutex<Connection>` for SQLite access. Uses `with_conn()` async accessor. Data persisted in `kronn.db` with WAL mode and foreign keys enabled.

**Migration** — Versioned SQL schema evolution in `backend/src/db/migrations.rs`. SQL files in `backend/src/db/sql/` (e.g., `001_initial.sql`, `002_mcp_redesign.sql`). Run before Mutex wrap to avoid async runtime issues.

**Encryption** — AES-256-GCM encryption for MCP secrets (env vars). Key derived from `encryption_secret` in config.toml via `core/crypto.rs`.

**DbInfo** — Response from `GET /api/config/db-info`: database file size and record counts per table.

**DbExport** — Full JSON dump of all database tables, retrieved via `GET /api/config/export` and restored via `POST /api/config/import`.

**full_access** — Boolean field on `AgentConfig` (persisted in config.toml). When true, agent runner adds `--dangerously-skip-permissions` (Claude) or `--full-auto` (Codex) to CLI invocations. Controlled via `GET/POST /api/config/agent-access`.

## Domain concepts

**Project** — A registered git repository managed by Kronn. Has MCPs, workflows, and AI config detection.

**Discussion** — A chat conversation with one or more AI agents, optionally tied to a project (`project_id: Option<String>`). Supports single-agent and multi-agent (orchestration) modes. Global discussions (no project) appear under "Général" in the sidebar.

**Orchestration** — Multi-agent debate: multiple agents discuss in rounds (max 3). Primary agent speaks last and synthesizes.

**MCP (Model Context Protocol)** — Standardized protocol for giving AI agents access to tools/data. Kronn uses a 3-tier model: servers → configs → project linkages.

**McpServer** — A known MCP server type (e.g. "GitHub"). Has id, name, description, transport, and source (Registry, Detected, Manual). Stored in `mcp_servers` table.

**McpConfig** — A configured instance of an MCP server with encrypted env vars, label, and optional args override. One server can have multiple configs (e.g. two GitHub configs with different tokens). Stored in `mcp_configs` table.

**McpConfigDisplay** — Read-only projection of McpConfig with masked secrets, server name, and linked project names. Used in API responses.

**McpDefinition** — A template MCP from the built-in registry (name, transport, env_keys, tags, token_url, token_help). 19 official servers grouped by category (Git, Databases, Cloud, Search, Monitoring, Communication, Project Management, Utilities). `token_url` links to the provider's token generation page; `token_help` provides a short description.

**McpInstance** — Legacy type kept for backward compatibility in the Project struct.

**Config hash** — FNV-1a hash of (transport + args + env values) used to deduplicate identical MCP configs.

**MCP disk sync** — When project-MCP linkages or config values change, Kronn writes agent-specific config files: `.mcp.json` (Claude Code, per-project), `.vibe/config.toml` (Vibe, per-project), `~/.codex/config.toml` (Codex, global). Ensures files are in `.gitignore`. Key naming: single config → `server.name.to_lowercase()`, multiple configs → `config.label`. Codex keys are slugified (`^[a-zA-Z0-9_-]+$`). Codex only gets stdio MCPs (SSE/streamable skipped). Codex global config preserves non-MCP settings.

**MCP context file** — Per-project instructions for AI agents using a specific MCP. Stored at `ai/operations/mcp-servers/<slug>.md`. Auto-created with default template on first sync; customized files are injected into agent system prompts. Managed via `McpPage.tsx` context editor modal.

**MCP injection** — `read_all_mcp_contexts()` in `core/mcp_scanner.rs` reads `.mcp.json` and MCP context files, then generates a prompt section listing all available MCP servers. Injected into agent prompts for workflow steps and discussions so agents use `mcp__<server>__<tool>` tools instead of Bash workarounds.

**customized_contexts** — `Vec<String>` of `"slug:projectId"` pairs in `McpOverview` where the context file has been customized (not default template). Used by frontend to color FileText icons.

**AgentType** — Enum: `ClaudeCode`, `Codex`, `Vibe`, `Custom`. Determines which CLI to spawn. `OpenCode` planned.

**DetectedRepo** — A git repository found by the scanner in configured scan paths.

**AiConfigStatus** — Detection of existing AI config files in a project (CLAUDE.md, .cursor/, .ai/, etc.).

**AiAuditStatus** — Enum: `NoTemplate`, `TemplateInstalled`, `Audited`, `Validated`. Computed from filesystem state (not stored in DB). Detected by `scanner::detect_audit_status()`.

**ai_todo_count** — Number of `<!-- TODO -->` markers remaining in `ai/*.md` files. Computed on-the-fly by `scanner::count_ai_todos()`, exposed on `Project` struct.

**Bootstrap prompt** — Block injected into `ai/index.md` between `KRONN:BOOTSTRAP:START` and `KRONN:BOOTSTRAP:END` markers. Instructs AI agents to analyze the repo and fill the `ai/` skeleton. Removed before running the automated audit.

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

**OpenCode** — Planned agent support.

**Agent Runner** — `backend/src/agents/runner.rs` — spawns CLI processes and streams stdout.

## UI

**Dashboard** — Main shell component (`Dashboard.tsx`) with tabs: Projets, Discussions, MCPs, Workflows, Config. MCP tab delegates to `McpPage.tsx`, Workflows tab delegates to `WorkflowsPage.tsx`.

**Setup Wizard** — First-run flow (`SetupWizard.tsx`) for configuring scan paths, detecting agents, and API tokens.

**Config** — Unified config tab: API tokens, output language, agent detection + permissions (full_access toggle), DB management (size, counts, export/import). (Agents tab merged into Config.)

**@mention** — Chat feature to target a specific agent (e.g., `@claude`) with autocomplete.

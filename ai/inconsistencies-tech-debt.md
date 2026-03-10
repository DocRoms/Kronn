# Inconsistencies & tech debt (index)

> Entry point: `ai/index.md`. Details: `ai/tech-debt/<ID>.md`.

## Purpose
- A shared list (human + AI readable) of **known inconsistencies** and **things that should be improved**.
- This file is **track-only** — it exists to prevent large sweeping changes by AI and to help create tickets.
- **Details are in individual files** under `ai/tech-debt/`. Only load a detail file when working on that specific topic.

## How to add an entry
1. Create `ai/tech-debt/TD-YYYYMMDD-short-slug.md` using the template below.
2. Add a one-line summary to the list in this file.

## Entry template (for detail files)
- **ID**: TD-YYYYMMDD-short-slug
- **Area**: (e.g. Backend | Frontend | CI | Config | Docs | Other)
- **Problem (fact)**: ...
- **Why we can't fix now (constraint)**: ...
- **Impact**: dev friction | test fragility | perf | security | correctness | docs
- **Where (pointers)**: files/paths/targets
- **Suggested direction (non-binding)**: ...
- **Next step**: ticket link or 'create ticket'

## Current list

| ID | Problem | Area | Priority |
|----|---------|------|----------|
| ~~TD-20260306-in-memory-storage~~ | ~~All data lost on restart~~ — **DONE**: SQLite persistence (`kronn.db`) | Backend | Done |
| ~~TD-20260308-mcp-redesign~~ | ~~Old flat MCP model~~ — **DONE**: 3-tier model (servers→configs→projects), encrypted secrets, registry matching, disk sync | Backend + Frontend | Done |
| ~~TD-20260306-mcp-noninteractive~~ | ~~Agents don't load MCP servers~~ — **DONE**: All 3 agents synced: Claude Code (`.mcp.json`), Codex (`~/.codex/config.toml`), Vibe (`.vibe/config.toml`) | Backend | Done |
| ~~TD-20260308-codex-vibe-mcp~~ | ~~Codex and Vibe use TOML config~~ — **DONE**: Disk sync now writes all 3 formats (JSON + 2x TOML). Codex global config preserves non-MCP settings. Vibe per-project. | Backend | Done |
| TD-20260306-monolith-dashboard | Dashboard.tsx ~2250 lines — MCP extracted to McpPage.tsx (~715 lines), Workflows to WorkflowsPage.tsx (~1660 lines). Discussions page next candidate. | Frontend | Low |
| TD-20260306-no-tests | Frontend: **DONE** — Vitest 4 + testing-library (9 suites, 71 tests, coverage-v8). Backend: no unit/API tests. Shell scripts (`lib/*.sh`): no tests (bats-core recommended). | Backend + Shell | Medium |
| TD-20260306-inline-styles | All styles are inline — no theming or consistency system | Frontend | Low |
| ~~TD-20260307-agents-tab~~ | ~~Agents tab should be merged into Settings~~ — **DONE**: merged into Config page | Frontend | Done |
| TD-20260307-tasks-to-workflows | Legacy scheduled tasks → Workflows engine — **DONE**: Full engine (models, DB, API, template, workspace, steps, runner, triggers, GitHub tracker, frontend UI with 5-step wizard + detail + runs + live SSE progress + run deletion). Actions wizard step removed (agents use MCP tools in steps instead). Remaining: Symphony WORKFLOW.md import. | Backend + Frontend | Done (core) |
| TD-20260307-docker-file-perms | Agents run as root in Docker — files created on host volumes owned by root:root (partially mitigated: resolve_host_path Docker mount priority fix) | Backend | Medium |
| ~~TD-20260307-global-discussions~~ | ~~Discussions always tied to a project~~ — **DONE**: `project_id` now optional, "Général" group in sidebar | Backend + Frontend | Done |

## Implementation roadmap

1. ~~**SQLite persistence**~~ — **DONE**: `kronn.db` with WAL mode, migrations framework, CRUD in `backend/src/db/`
2. ~~**Merge Agents into Settings**~~ — **DONE**: merged into Config page as a card, with full_access toggle
3. ~~**Global discussions**~~ — **DONE**: `project_id` optional, "Général" group, agent runs in temp dir
4. ~~**MCP redesign**~~ — **DONE**: 3-tier model (servers→configs→projects), encrypted secrets, built-in registry (19 servers with token_url/token_help), auto-detection/matching, disk sync, editable labels, duplicate configs, categorized registry UI, inline secret editing with per-field visibility, MCP context files (ai/operations/mcp-servers/*.md) with customization detection
5. **Docker file permissions** — Pass host UID/GID, run agents with `--user` (partially mitigated)
6. **Workflow engine** — See detailed plan below

---

## Workflow engine plan (TD-20260307-tasks-to-workflows)

### Phase 1 — Backend engine

**Step 1: Data models** (`models/mod.rs` + migration `003_workflows.sql`)
- `Workflow { id, name, project_id, trigger, steps, actions (deprecated — always empty), safety, enabled, created_at, updated_at }`
- `WorkflowTrigger` — enum: `Cron { schedule }`, `Tracker { source, query, interval, labels }`, `Manual`
- `WorkflowStep { name, agent, prompt_template, mode, mcps, on_result }`
- `StepMode` — enum: `Normal`, `Debate { agents, max_rounds }`
- `WorkflowAction` — enum (deprecated, kept in model for backward compat, removed from UI wizard): `CreatePr`, `CommentIssue`, `UpdateTrackerStatus`, `CreateIssue`
- `WorkflowSafety { sandbox, max_files, max_lines, require_approval }`
- `WorkflowRun { id, workflow_id, status, trigger_context, step_results, tokens_used, workspace_path, started_at, finished_at }`
- `RunStatus` — enum: `Pending`, `Running`, `Success`, `Failed`, `Cancelled`, `WaitingApproval`
- `StepResult { step_name, status, output, tokens_used, duration_ms }`
- `StepCondition { on_result: Vec<ConditionRule> }` — e.g. `{ contains: "NO_RESULTS", action: Stop }`
- `ConditionAction` — enum: `Stop`, `Skip`, `Goto(step_name)`
- **Symphony-compatible additions:**
  - `WorkspaceConfig { hooks: WorkspaceHooks }` — lifecycle hooks: `after_create`, `before_run`, `after_run`, `before_remove` (shell commands)
  - `AgentSettings { model, reasoning_effort, max_tokens }` — per-step agent configuration override
  - `concurrency_limit: Option<u32>` on Workflow — max simultaneous runs
  - `retry: Option<RetryConfig>` — `{ max_retries, backoff: exponential }` for failed steps
  - `stall_timeout: Option<Duration>` — kill step if no output for N seconds

**Step 2: DB persistence** (`db/workflows.rs`)
- CRUD operations for Workflow and WorkflowRun
- Workflows stored as JSON blobs (trigger, steps, actions, safety)
- Runs indexed by workflow_id + status + started_at

**Step 3: Template engine** (`workflows/template.rs`)
- Liquid-compatible template rendering (Kronn superset of Symphony)
- Built-in variables: `{{issue.title}}`, `{{issue.body}}`, `{{issue.number}}`, `{{issue.url}}`, `{{issue.labels}}`
- Step chaining: `{{previous_step.output}}`, `{{steps.<name>.output}}`
- Custom variables from trigger context

**Step 4: Workspace management** (`workflows/workspace.rs`)
- `git worktree add` for isolated execution
- Lifecycle hooks execution (after_create, before_run, after_run, before_remove)
- Cleanup on completion/failure
- Branch naming: `kronn/<workflow-name>/<run-id>`

**Step 5: Step execution** (`workflows/steps.rs`)
- Resolve per-step MCPs → sync to disk before agent runs
- Call agent runner with rendered prompt
- Capture output for `{{previous_step.output}}` chaining
- Handle `on_result` conditions (stop/skip/goto)
- Respect stall timeout
- Retry with exponential backoff on failure

**Step 6: Workflow runner** (`workflows/runner.rs`)
- Orchestrate full run: create workspace → execute steps sequentially → cleanup
- SSE streaming for real-time progress (`RunEvent`: `StepStart`, `StepDone`, `RunDone`, `RunError`)
- Concurrency limiting (respect `concurrency_limit`)
- Token accounting per step and per run
- MCP tools auto-injected into agent prompts via `read_all_mcp_contexts()`

**Step 7: Trigger system** (`workflows/trigger.rs`)
- Cron evaluation (next_tick check)
- Tracker polling with reconciliation (track processed issue IDs to avoid duplicates)
- Manual trigger via API

**Step 8: Tracker adapters** (`workflows/tracker/`)
- `TrackerSource` trait: `poll_new_items()`, `update_status()`, `comment()`, `create_pr()`
- GitHub implementation first (`tracker/github.rs`) — GitHub API v3
- Linear, GitLab, Jira as future community PRs

**Step 9: API endpoints** (`api/workflows.rs`)
- `GET /api/workflows` — list all workflows
- `POST /api/workflows` — create workflow
- `PUT /api/workflows/:id` — update workflow
- `DELETE /api/workflows/:id` — delete workflow
- `POST /api/workflows/:id/trigger` — manual trigger
- `GET /api/workflows/:id/runs` — list runs
- `GET /api/workflows/:id/runs/:run_id` — run details (SSE for active runs)
- `DELETE /api/workflows/:id/runs` — delete all runs for a workflow
- `DELETE /api/workflows/:id/runs/:run_id` — delete a single run
- `POST /api/workflows/import` — import from WORKFLOW.md (Symphony compatible)

**Step 10: WorkflowEngine** (`workflows/mod.rs`)
- Background polling loop (tick every 30s)
- Check all enabled workflows' triggers
- Spawn runs, enforce concurrency limits
- Register in AppState, start on boot

### Phase 2 — Frontend UI

**Step 1: Workflow list page** (new tab in Dashboard, replaces Tasks)
- List of workflows with status indicators (enabled/disabled, last run status)
- Create button → wizard

**Step 2: Workflow creation wizard**
- 5-step form: infos (name + project) → trigger config → steps (add/reorder) → config (safety + workspace + concurrency) → resume
- Per-step: choose agent, write prompt (with template variable hints), optional debate mode, conditional branching (`on_result`), retry, stall timeout
- Trigger-specific config: cron expression builder (visual every/unit/at), tracker query builder (owner/repo/labels), or manual
- Actions removed from wizard — agents handle post-step operations (create PR, comment, etc.) via MCP tools within steps

**Step 3: Run monitoring**
- Real-time SSE view of active runs (step progress, agent output streaming)
- Run history with logs, token usage, duration
- Cancel button for active runs
- Approval gate UI (approve/reject for runs waiting approval)

**Step 4: WORKFLOW.md import**
- File picker or paste content
- Parse Symphony WORKFLOW.md → map to single-step Kronn workflow
- Show preview, let user adjust before saving
- Auto-detect missing MCPs and suggest installation from registry

### Symphony compatibility (WORKFLOW.md import mapping)

A Symphony WORKFLOW.md maps to a **single-step** Kronn workflow:
- `agent_name` → step agent (always Codex in Symphony, but Kronn allows any)
- `prompt` → step prompt_template
- `triggers[].type=tracker` → `WorkflowTrigger::Tracker`
- `triggers[].type=cron` → `WorkflowTrigger::Cron`
- `triggers[].tracker_query` → tracker labels/query
- `concurrency` → `concurrency_limit`
- Symphony's `workspace.hooks` → `WorkspaceConfig.hooks`
- Symphony's `model`, `reasoning.effort` → `AgentSettings`

**Symphony concepts integrated into Kronn:**
1. Workspace hooks (after_create, before_run, after_run, before_remove)
2. Concurrency limits (max simultaneous runs per workflow)
3. Exponential backoff retry for failed steps
4. Tracker reconciliation (dedup processed issues, avoid re-runs)
5. Stall detection (timeout if agent produces no output)
6. Liquid-compatible template engine (`{{issue.title}}`, etc.)
7. Token accounting (per-step and per-run totals)

### Reference use case: 5xx auto-fix workflow

```
name: "Auto-fix 5xx errors"
trigger: Tracker (GitHub, label: "bug-5xx", interval: 5min)
steps:
  1. name: "analyze"
     agent: claude
     prompt: "Analyze the 5xx error in {{issue.title}}. Read the logs, find root cause."
  2. name: "fix"
     agent: claude
     prompt: "Based on analysis: {{steps.analyze.output}}. Write the fix."
     on_result: [{ contains: "NO_RESULTS", action: stop }]
  3. name: "verify"
     agent: codex
     prompt: "Run tests to verify the fix. Report results."
  4. name: "submit"
     agent: claude
     mcps: [github]
     prompt: "Create a draft PR with title 'fix: {{issue.title}}' on branch 'fix/{{issue.number}}'. Comment on the issue with the PR URL."
safety: { sandbox: true, max_files: 10, require_approval: false }
```

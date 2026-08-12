# UI structure

Canonical description of the app's pages, tabs and component layout.

Moved out of `docs/AGENTS.md` (KT-191): it weighed 53 167 bytes — 63,6 % of a
file every session reads in full — while being needed only when actually
touching the UI. The content below is unchanged; load it on demand.

Dashboard tabs (current / planned):

| Tab | Status | Content |
|-----|--------|---------|
| Projets | Done | Master/detail project workspace with independent list/detail scrolling, filters and sorting. Per-project views: overview, discussions, tasks, docs/audit, resources and **Code**. Tasks lists explicit project work plus tasks inherited through a linked project discussion, supports quick creation/completion, opens the global backlog and deep-links each row to its full Planning detail. Discussion/task totals are shown in their tabs; Discussions renders the ten newest entries first and can load 10, 50 or all older entries. `[src: file: backend/src/db/planning.rs:430-460]` `[src: file: frontend/src/components/ProjectCard.tsx:115-145]` `[src: file: frontend/src/components/ProjectCard.tsx:1760-1790]` `[src: file: frontend/src/components/ProjectCard.tsx:1839-1898]` Code switches between the read-only source explorer and an on-demand syntax-aware Git diff with separate working-tree and committed-on-branch files. `[src: file: frontend/src/components/ProjectTasksPanel.tsx:1-200]` `[src: file: frontend/src/components/ProjectCodePanel.tsx:1-168]` The overview exposes browser-safe repository + PR/MR shortcuts, branch/latest-tag/upstream state, local changes, a bounded language breakdown and cached dependency-update health for JavaScript, Composer, Cargo, Go, Bundler, NuGet, Poetry and Gradle manifests. Dependency checks prefer bounded read-only manager commands; failed or unavailable runtimes share one pinned Renovate local dry-run fallback, including Gradle/Android and locked Bundler projects without a matching host toolchain. Failures remain explicit, results expire after six hours or any manifest/lockfile change, and the UI supports a forced refresh. When the host has no Composer, Kronn first probes an already-running Docker Compose service, then falls back to the official Composer image with the manifest directory mounted read-only; neither path starts the project stack. Git working-tree/branch state always stays live, while only the heavier language breakdown is cached for one hour (invalidated by source-exclusion changes) and exposes a compact checked-at/refresh control. The source explorer uses Rust `source-files` / `source-file` / `source-search` / `source-exclusions` / `git-blame` endpoints, discovers UTF-8 text files regardless of extension, marks Git-ignored entries, and excludes docs/dependencies/build outputs/binaries/obvious secret files. `source-files?shallow=true` returns repository-root entries for immediate rendering while the UI fetches the complete bounded tree in the background; Git-ignore lookup writes candidates concurrently with output collection so large ignored sets cannot deadlock on a full pipe. Common cache/vendor/bundle folders are skipped by default; any visible folder can be excluded per project and restored from the UI, and source search has a bounded byte budget. The overview language summary reuses the same exclusions. Source browsing adds curated highlight.js syntax colours, cross-file occurrence navigation, the current Git branch and optional author/date annotations per line. Also hosts the AI audit pipeline (template → audit → validation), project bootstrap, MCP overview and per-project workflows/skills. |
| Discussions | Done | Single/multi-agent chat, @mentions, orchestration, global discussions, archive/unarchive (swipe gestures), inline title editing, disabled agent detection. **⏹ Stop agent** button (CancellationToken via `AppState.cancel_registry`, CancelGuard RAII). **Partial response recovery (0.3.5)**: agent output checkpointed every ~30s/~100 chunks into `discussions.partial_response` (+ `partial_response_started_at` for chronological order). Backend restart converts dangling partials into Agent messages with "⚠️ Réflexion interrompue" footer + broadcasts `WsMessage::PartialResponseRecovered`. `POST /api/discussions/:id/dismiss-partial` for manual recovery. `send_message` refuses a new run while a partial is pending (`partial_pending` SSE error → frontend waits or dismisses). **Structured agent questions (0.3.5)**: `{{var}}: question` patterns in agent messages auto-render a mini-form (`AgentQuestionForm`) above ChatInput. **0.5.0 — Test mode (worktree swap-in-main)**: `🧪 Tester cette version` CTA in the ChatHeader swaps the main repo to the discussion's branch, global banner stays pinned while active, single-click exit restores previous branch + pops auto-stash + re-creates the worktree. Triple preflight (worktree dirty, main dirty, detached HEAD) with a dedicated modal for the MainDirty case (stash-and-proceed / commit-first / cancel). `POST /api/discussions/:id/test-mode/{enter,exit}` return tagged envelopes. Persistent across reboots via migration 034. **0.5.0 — Decoder-loop detection**: agent streams now kill the child after 50 consecutive identical non-whitespace deltas (fixes Claude Opus extended-thinking `</thinking>`-loop leaking 76 KB into one response on EW-7189). Parser-level strip of literal `<thinking>` tags is the first line of defense. **0.5.0 — Prompt over stdin for Claude Code**: `start_agent_with_config` writes the prompt to `stdin` instead of argv, bypassing Linux `ARG_MAX` (~128 KiB). `--append-system-prompt` still travels via argv but truncates at 100 KiB with a clear marker. **Split into 8 components**: DiscussionsPage (orchestrator) + ChatHeader, ChatInput, DiscussionSidebar, NewDiscussionForm, MessageBubble, SwipeableDiscItem, AgentQuestionForm — plus 2 test-mode components (`TestModeBanner`, `TestModeModal`) in 0.5.0. **0.8.12 — owed-run tracking**: `discussions.awaiting_agent` (migration 074) is set inside `make_agent_stream` AFTER every preflight early-return (plus at batch enqueue), cleared only on delivery/error/deliberate cancel; a 3rd boot-reconcile (`reconcile_awaiting_agents`, after partial recovery) appends an interruption notice + broadcasts `AgentRunsInterrupted` — **never** a re-spawn. `DELETE /discussions/:id` fires the disc's cancel token before the DB delete. Sidebar: live-first ordering (running > queued ⏳ > recency, applied before the per-project display cap), a collapsible cross-project Favorites section, and a compact multi-selection mode for bulk archive/delete with one confirmation. `[src: file: frontend/src/components/DiscussionSidebar.tsx:180-214]` `[src: file: frontend/src/components/DiscussionSidebar.tsx:409-495]` **0.9.2 — model provenance (KT-37):** a reply records the concrete model it ran on, resolved once before spawn and reused on non-zero exit / stall / cancel `[src: file: backend/src/api/discussions/streaming.rs:853-858]` and on a genuine spawn error (the System error message carries agent + attempted model for a provenance label) `[src: file: backend/src/api/discussions/streaming.rs:1936-1952]`; a mid-stream checkpoint now carries agent + model so a restart-recovered partial is attributed, and legacy pre-089 checkpoints degrade to an anonymous bubble `[src: file: backend/src/db/discussions.rs:194-240]`; orchestration rounds + synthesis stamp the tier-resolved model, and an owed-but-never-started run invents none. A JOIN peer may self-DECLARE the model it runs on via `disc_join(model?)` → `peer-join`: optional, trimmed, bounded (an over-long declaration is refused, never truncated) `[src: file: backend/src/api/disc_invite.rs:160-173]`, recorded as **declared-at-join** — durable and never inferred as a live value; an explicit value on a later join updates it, an omitted one preserves the prior declaration `[src: file: backend/src/api/disc_invite.rs:205-228]` `[src: file: backend/src/db/discussion_sessions.rs:292-300]`, surfaced on `DiscussionSession` + `ParticipantView` (migration 089). `[src: file: backend/src/db/sql/089_agent_model_provenance.sql:1-19]` |
| Plugins | Done | Plugin registry — card grid + category pills, inline detail panel, per-project navigation. **0.5.0 — plugin kind: MCP \| API \| hybrid** (see the plugin kind table below). **56 plugins: 53 MCPs + 3 API plugins** (Chartbeat `apikey` query, Adobe Analytics OAuth2 S2S, Google Search `apikey` query). Per-card badges `🔌 MCP` / `🌐 API` / `MCP + API`. Kind-filter pills `All \| MCP \| API` on top. Publisher origin badges (official/community). Per-project MCP load indicator (green/orange/red). Env placeholders with realistic hints (sourced from `api_spec.config_keys` for API plugins, else static map). Eye toggle on add form; API plugins auto-expose non-secret fields as plain text with inline description. OAuth2 plugins get Kronn-managed bearer refresh transparent to the agent. Recent additions: MongoDB, Kubernetes, Qdrant, Perplexity, Microsoft 365, Chartbeat, Adobe Analytics, Google Search. Puppeteer removed (use Playwright). **0.9.2 — detail opens as a non-blocking right-hand panel** (`<aside data-testid="mcp-plugin-panel">`, no backdrop, `aria-modal` absent), mechanics shared with the discussion tool panels: the card grid stays interactive and reflows beside the panel (`.mcp-page-with-panel`), so clicking another card swaps the content in one click; the panel is offset below the sticky nav from its measured height, and `Escape` closes. Readiness is a **real probe** (`POST /api/mcps/configs/:id/probe`) rendered per check with a server-provided `required` flag, so an optional capability missing (e.g. `fastly-mcp` outside Docker) doesn't mark the whole plugin broken. `[src: file: frontend/src/pages/McpPage.tsx:2278-2290]` `[src: file: frontend/e2e/specs/plugin-detail-panel.spec.ts:53-75]` **Portable selections (KT-53):** multi-select export is configuration-only by default. Including values requires the red-zone confirmation phrase and a passphrase, and encrypts the complete payload; CLI-resolved credentials never leave the machine. Import resolves registry definitions locally, refuses unknown executables, undeclared environment keys and silent overwrites, drops every source scope, and is idempotent + audited. `[src: file: backend/src/api/plugin_portability.rs:351-797]` `[src: file: frontend/src/components/PluginPortabilityModal.tsx:28-337]` |
| Automatisation | Done | Two tabs: **Workflows** + **Quick Prompts**. Workflows: list (grouped by project), creation wizard (simple 3-step + advanced 5-step), detail + runs with live SSE, manual trigger, run deletion. MCP-based suggestions (10 templates). Structured inter-step contracts. AI Architect ("Create with AI" → discussion → `KRONN:WORKFLOW_READY`). Test step (dry-run + live streaming, state survives tab switches via module-level tracker). Starter templates (6 examples). Raw cron editor. **⏹ Cancel run** with cascade to child batch discussions via `parent_run_id`. **Notify step (0.3.5)**: `StepType::Notify` with webhook support (POST/PUT/GET), zero tokens, template rendering in URL + body. **Quick Prompts**: reusable prompt templates with `{{variables}}` and conditional sections `{{#var}}text{{/var}}`. Launch creates a discussion with rendered prompt and dynamic title. **Batch Quick Prompts (0.3.5)**: fan-out to N items (tickets / list / resolved template), each child gets its own discussion + optional worktree, aggregated in sidebar groups. Dry-run preview with per-item rendered prompt + per-item test button. **Active-runs popover (0.5.1)**: `ActiveRunsPopover` hijacks the nav-icon click when `runningWorkflows > 0` — renders a tray listing every live run (workflow name + project + live elapsed timer + one-click `⏹ Arrêter`) without leaving the current page. Inline `⏹ Stop` button also present on every `.wf-card` whose `last_run.status === Running \| Pending` (`stopPropagation` guards against opening the detail panel). `onMouseDown` stop on the nav button prevents the outside-click-closes-popover race. **0.6.0 — Workflow Engine 0.7.x step types**: `StepType::Gate` (human approval, `WaitingApproval` run status, `POST /api/workflows/.../decide`), `StepType::Exec` (allowlisted binary, argv literal, no shell), plus `WorkflowGuards` (timeout / max LLM calls / loop revisits → `StoppedByGuard`), artifacts (`---ARTIFACT:name---` → workspace files), durable run state (`---STATE:k=v---` + `{{state.X}}` + `{{iter.X}}`), `ConditionAction::Goto` loops, `on_failure` rollback steps, per-item Workflow + QP Export/Import. See the Workflow Engine 0.7.x features table below. **`StepType::ApiCall` (0.5.1 — désagentification)**: workflow step that hits an API plugin directly from the Rust engine (0 tokens), extracts JSON via `serde_json_path` (RFC 9535), pipes to the next step. Full wizard card (`ApiCallStepCard.tsx`) with plugin+endpoint pickers, Test button, clickable JSON tree for click-to-pick path generation, live preview debounced 150 ms via `/api/workflow-steps/test-extract`, 3 example-path buttons, next-step-batch compatibility banner, advanced options collapsible (timeout/retries/output_var/fail_on_empty). Starter template "Chartbeat top 5 → Agent résumé → Slack" cloneable from the wizard. Security triple-guard: SSRF host allowlist, DNS rebind block, `ResolvedAuth` redact (Bearer / api-keys / OAuth2). Auto-pagination shape detection (Jira offset / CF cursor / Stripe has_more / Jira v3 nextPageToken). See [`docs/operations/deagent-apicall.md`](operations/deagent-apicall.md). **0.8.12 — honest batch-child states**: `BatchRunChildQueued` broadcast up-front vs `BatchRunChildStarted` fired only after the global agent-semaphore permit (⏳ queued vs ▶ running; the queued state is DB-backed via `awaiting_agent` on the Discussion DTO so it survives reloads and WS frames missed while another page was mounted); a child that fails preflight or never starts still bumps the batch counters, so runs can't stick at n-1/N. `delete_batch_run` AND `cancel_run` cascade to direct children (`workflow_run_id = run_id`, not only `parent_run_id`); a deliberate cancel clears the awaiting marker of unstarted children. **0.8.12 wf-ux**: sticky frosted-glass save bar in the wizard, copyable short-id pill in the detail header, and **workflow favorites** (migration 075) — star toggle on cards, pinned workflows in a collapsible cross-project Favorites group at the top of the list. |
| Config | Done | Multi-key API management (incl. Mistral/Vibe API keys), token usage tracking, language, agent detection + permissions, agent usage dashboard links, Directives CRUD with live cards, DB management (**export ZIP** with data.json + config.toml, **import ZIP/JSON** with config merge + path remapping). **Global context (0.3.5)**: markdown textarea + mode dropdown (always/no_project/never), injected into agent prompts via `ServerConfig.global_context`. Skills/Profiles are now managed per-project on the Project page. **Skill auto-trigger opt-out (0.5.1)**: per-skill toggle backed by `auto_triggers` table — disable a skill from contributing to prompt injection without removing it. **RTK integration (0.5.1)** in the Agents section: `<CompressionSection />` card at the top with 3 activation states, one-click "Activate on all compatible agents" CTA, install modal with copy-paste curl when the binary is absent, live savings counter + 3-card expand (tokens / ratio / samples), sobriety (?) tooltip nuancing the "eco mode" label. Per-agent badge inline next to the version: 🟢 `RTK actif` / 🟡 hook missing / ⚪ not installed / italic `Non pris en charge par RTK` (Kiro, Copilot CLI, Vibe). See [`rtk-integration.md`](rtk-integration.md). |
| Planification | Done (0.9.2) | Shared ranked tasks/subtasks, DoD, links/tags/blockers, project/discussion relations, attributable activity, global backlog, discussion side panel, compact MCP reads/writes, delta-only context notice and human-gated agent proposal cards. `[src: file: docs/design/planning-and-discussion-plans.md:1-181]` **Durable proposals (0.9.2):** an agent's `kronn-plan-action` fence is parsed and persisted the instant its message is stored — a proposal exists in the inbox even if nobody opened the message — then validated item by item by a human. `[src: file: backend/src/db/planning_proposals.rs:358-410]` Acceptance applies the underlying task mutation + discussion link in one transaction, idempotently: a retry with the same key returns the same result/receipt (never a duplicate) and a contradictory decision on an already-terminal item is a 409. `[src: file: backend/src/db/planning_proposals.rs:927-1010]` A light, non-error `[kronn-planning: …]` System receipt is written in the same transaction. The discussion-plan panel shows a validation inbox with a pending header counter. Contract: **agents PROPOSE, only a human DECIDES** — the MCP surface is strictly read-only (`proposal_list`, `proposal_get`; no accept/reject/decide tool exists). `[src: file: backend/scripts/disc-introspection-mcp.py:3144-3165]` `[src: file: backend/src/lib.rs:717-732]` **Plan focus + all-tasks view (0.9.2 KT-30):** `get_discussion_plan` builds a compact projection in a FIXED number of query families — batched summaries + a batched active-dependency pass, no per-task N+1. `[src: file: backend/src/db/planning.rs:1149-1252]` `[src: file: backend/src/db/planning.rs:1254-1394]` Each relation carries `active_blockers` (minimal, read-only) + an `actionable` flag, and the plan carries a strict-precedence `stats` bucketing (done > blocked > in_progress > ideas > ready; the five Active buckets sum to `total_active`). `[src: file: backend/src/models/planning.rs:248-272]` The panel opens in **Focus** (primary objective + ≤3 in-progress + the 5 first `actionable` in plan order + a compact ready/blocked/done summary) `[src: file: frontend/src/components/DiscussionPlanPanel.tsx:140-152]` and toggles to a searchable, TanStack-virtualised **all-tasks** view with roving `listbox`/`option` a11y and non-focusable headers. `[src: file: frontend/src/components/PlanAllTasksView.tsx:102-112]` `[src: file: frontend/src/components/PlanAllTasksView.tsx:194-252]` Its DOM stays bounded at 200/1000 rows. `[src: file: frontend/src/components/__tests__/PlanAllTasksView.test.tsx:160-168]` Selecting a task reveals a dependency neighbourhood (blockers → task → blocked tasks) with honest links to an external blocker's own discussion/project. `[src: file: frontend/src/components/DiscussionPlanPanel.tsx:638-663]` |

Note: the old "Agents" tab has been merged into Config. Nav order: Projets → Discussions → Planification → Plugins → Workflows → Config. **"?" button** in nav replays the guided tour.

**Guided tour (0.3.6)**: 17-step interactive onboarding auto-launched on first visit. 5 acts (Projets → Plugins → Discussions → Automatisation → Config). 4 interactive steps with `waitForClick` (user must click the real UI element — pulse animation, "Next" blocked). Spotlight via box-shadow cutout, tooltip auto-positioned. Ends on Discussions page. Persistence: `kronn:tour-completed` in localStorage. Components: `TourProvider` (context + state machine), `TourOverlay` (portal), `tourSteps.ts` (declarative step definitions), `useTourPositioning.ts` (placement + MutationObserver).

### Project Bootstrap (create from scratch)

`POST /api/projects/bootstrap` — creates a new project directory, initializes git, installs AI template, creates a bootstrap discussion with architect + product-owner + entrepreneur profiles (3 profiles). **Bootstrap++**: skill `bootstrap-architect` auto-injected for gated validation flow:
1. Agent reads uploaded context files (architecture docs, specs, PRDs) → produces architecture summary → `KRONN:ARCHITECTURE_READY` → CTA validates
2. Agent generates project plan (epics, stories, estimates) → `KRONN:PLAN_READY` → CTA validates
3. Agent creates issues on tracker via MCP → `KRONN:ISSUES_CREATED` → CTA navigates to project

Frontend modal includes **drag & drop file upload** for documents. Files uploaded as context files after discussion creation. `BootstrapProjectRequest` accepts `skill_ids` for skill injection.

### Pre-audit briefing (optional)

`POST /api/projects/:id/start-briefing` — creates a briefing discussion where the AI asks 5 quick questions (project purpose, stack, team, conventions, watch points). The agent writes `docs/briefing.md` and emits `KRONN:BRIEFING_COMPLETE`. The briefing content is injected into each audit step via `PROMPT_PREAMBLE`. Agents without filesystem access (Vibe) are excluded from briefing/audit.

### CI pipeline

GitHub Actions workflow (`.github/workflows/ci-test.yml`) triggered on push to `main` + all PRs:
- `test-backend`: cargo clippy + cargo test (with sccache)
- `test-frontend`: tsc --noEmit + pnpm test (Node 24 LTS)
- `test-shell`: make test-shell (bats)
- `security-scan`: cargo audit + pnpm audit

`.github/workflows/dependency-review.yml` also runs weekly, on demand, and as a
required precursor to every tagged desktop build. It reports security audits
separately from ordinary version drift and only uses read-only/dry-run commands.

### AI audit pipeline (4-state badge system)

Projects display 3 badges next to the title: `[FileCode] Project docs`, `[Cpu] AI audit`, `[ShieldCheck] Validated`.

| State | Project docs | AI audit | Validated | Meaning |
|-------|-------------|----------|-----------|---------|
| NoTemplate | gray | gray | hidden | No `docs/` directory |
| TemplateInstalled | green | orange | gray | Template copied, audit pending |
| Audited | green | green | gray | Chained audit completed (9 docs steps + 7 sub-audits) |
| Validated | green | green | green | Validation discussion resolved all TODOs |

- **Template install**: copies `docs/` skeleton + redirector files (CLAUDE.md, .cursorrules, etc.) + injects bootstrap prompt
- **AI audit (chained, 0.9.0)**: one `full` launch runs the 9 docs steps then chains 7 sub-audits (Security, Docker, Performance, Accessibility, Database, ApiDesign, CodeQuality — RGAA stays on-demand), 16 steps total over SSE, ~35–40 min. Each sub-audit starts with a relevance gate: if the dimension does not apply to the project, the agent writes a one-line "Not applicable" and moves on. **Token cost: ~100K–250K tokens per full chained audit.** Fills all `docs/` files and appends per-dimension findings to the inconsistency docs. A drop-guard cleans up (tracker + DB row marked Interrupted) if the SSE stream is abandoned mid-run — same guard on the partial/drift path. Kronn-managed audit artifacts (the complete chained target set plus `docs/tech-debt/*.md`) are redacted fail-closed before the first agent, after every attempt before any retry, and before publication; linked validation discussions repeat the guard before and after their agent and cannot accept `KRONN:VALIDATION_COMPLETE` if the final sweep fails.
- **Validation**: opens a prefilled discussion (locked title/prompt) where the AI asks questions about ambiguities. AI updates `docs/` files after each answer. Project page shows "validation en cours" + link to discussion (no validate button on project page).
- When the AI finishes all questions, it includes `KRONN:VALIDATION_COMPLETE` in its last message. This triggers a green banner in the discussion with a "Marquer l'audit comme valide" button. Similarly, `KRONN:BRIEFING_COMPLETE` signals the end of a pre-audit briefing discussion. `KRONN:WORKFLOW_READY` signals the AI Architect has produced a deployable workflow JSON (extracted from ```json block → one-click creation). **Bootstrap++ signals**: `KRONN:ARCHITECTURE_READY` → validate architecture, `KRONN:PLAN_READY` → validate plan, `KRONN:ISSUES_CREATED` → view project. Each gate sends a user message to continue the agent.
- **Mark as validated**: injects `<!-- KRONN:VALIDATED:date -->` marker into `docs/AGENTS.md`.
- AI config file badges (CLAUDE.md, .cursorrules, etc.) shown on a second line below the status badges.
- **MCP-driven audits**: the `kronn-internal` bridge exposes `audit_prepare` (audit surface + current status + briefing state), `audit_launch` (full/partial; `steps` required for partial; `resume_run_id` full-only — resume an Interrupted run by id, the backend derives kind + checkpoint from the row and only accepts the project's most recent run), `audit_status` (bridge stream / live tracker / DB history, kept separate), `audit_install_template`, `bridge_info` (bridge staleness vs the script on disk) and `kronn_intro` (first-contact onboarding tour; per-client marker in `~/.config/kronn/mcp-onboarded.json`). An audit launched over MCP lives with the bridge session — if the stream is abandoned, the drop-guard reconciles state and the run stays resumable. The UI adopts externally-launched audits on the project card (fleet `GET /api/audit-status` poll) and a `WsMessage::AuditFinished` broadcast drives the end-of-audit toast + refetch.

### Audit drift detection

`GET /api/projects/:id/drift` — compares source file checksums against `docs/checksums.json` (generated during audit). Returns stale sections without consuming tokens. `POST /api/projects/:id/partial-audit` re-runs only stale steps (~3-5K tokens vs ~20K for full audit). Since the A5 hardening a partial run gets its own `audit_runs` row (kind `Partial`, structured `step_outcomes_json`), revokes any prior Validated state on start, and — when every requested step succeeds — atomically creates a validation discussion scoped to the refreshed sections (the project stays Audited until that discussion ends on `KRONN:VALIDATION_COMPLETE` and the user validates). UI shows an amber badge on stale projects with a "Mettre à jour" button.

**MCP drift auto-detection**: adding/removing/relinking a plugin on an audited project automatically invalidates the `.mcp.json` checksum, flagging drift for step 8 (MCP introspection) re-run.

### Workflow suggestions

`GET /api/projects/:id/workflow-suggestions` — matches installed MCPs against a hardcoded catalogue of 10 workflow templates. Returns suggestions with multi-step prompts, pre-filled triggers, and audience tags (dev/pm/ops). Suggestions use structured inter-step contracts for reliable data passing between collection and synthesis steps.

### Structured inter-step contract (canonical Kronn envelope, 0.8.5+)

**Every envelope-producing step type emits the same byte-for-byte shape via `workflows/step_output_format.rs::format_step_output`:**

```
[optional human-readable prefix line(s)]
---STEP_OUTPUT---
{"data": <any JSON>, "status": "OK|NO_RESULTS|ERROR|PARTIAL|PENDING|…", "summary": "<one line>"}
---END_STEP_OUTPUT---
[SIGNAL: <primary>]
[SIGNAL: <optional secondary>]
```

| Step type | Emits canonical envelope | Primary signal |
|---|---|---|
| `ApiCall`, `BatchApiCall` | yes | `OK` / `NO_RESULTS` / `ERROR` (+ `http_<code>` on HTTP errors) |
| `Exec` | yes | `OK` / `ERROR` + `exit_<code>` |
| `JsonData` | yes | `OK` |
| `Notify` | yes (0.8.5+) | `OK` / `ERROR` (0.8.5+, pre-fix none) |
| `BatchQuickPrompt` | yes (0.8.5+) | `OK` / `PARTIAL` / `ERROR` / `PENDING` (0.8.5+, pre-fix none) |
| `Agent` (Structured / TypedSchema) | yes — prompt template emits markers | whatever the prompt instructs |
| `Agent` (FreeText) | **no** — raw text only, consumers read `.output` only | whatever the agent prints |
| `Gate` | **no** — output is the rendered `gate_message`, has no semantic data | none — Gate is a pause, branch via `request_changes_target` |

`BatchQuickPrompt` additionally exposes a truthful latency and dispatch audit.
Each child result carries durable `queued_at`, `claimed_at`,
`agent_started_at`, `settled_at` and status fields. The batch envelope carries
monotonic active time, calendar wall time, estimated suspension, and the
10-minute target / 15-minute maximum / 20-minute hard active budget. At the
hard budget the step fails with `LATENCY_BUDGET_EXCEEDED`; one shared
cancellation transaction stops live child tokens, settles every pending or
running dispatch, clears every `awaiting_agent` marker and terminates the child
batch. Laptop suspension therefore remains observable without being mistaken
for model execution time. Migration 120 adds `agent_started_at`.

For Agent steps with `output_format: Structured` or `TypedSchema`, the engine auto-injects the envelope instructions into the prompt, and `extract_step_envelope` parses the result via marker-delimited strategy-1 (preferred) or last-bare-JSON-with-`data`+`status` strategy-2 (legacy back-compat for pre-0.8.5 run records). For all other step types, the runner writes the canonical envelope directly. `TypedSchema { schema, on_invalid: Continue|Fail }` adds JSON-Schema validation on top of the same envelope — failures fall back to a repair prompt with the schema diagnostic, then optionally fail the step.

Downstream consumers read the same access patterns regardless of producer:
- `{{steps.X.data}}` — JSON payload (compact for objects/arrays, string for scalars)
- `{{steps.X.data.<path>}}` — nested traversal (dot-separated, numeric segments index arrays). Missing fields leave the placeholder literal AND `find_unresolved_critical_refs` fails the consuming step with an actionable error.
- `{{steps.X.summary}}` / `{{steps.X.status}}` — the one-line summary / status string
- `{{steps.X.data_json}}` — always-serialized JSON, useful for piping into a downstream HTTP body
- `{{steps.X.output}}` — raw output (every step type, always available — fallback for Gate / FreeText consumers)

Cross-step transmission is pinned by the comprehensive matrix in `backend/src/workflows/template.rs::cross_step_transmission` (17 tests) — any step type that regresses its emitted shape fails one localised test instead of silently breaking every consumer.

See `backend/src/workflows/step_output_format.rs` (the single emitter), `backend/src/workflows/template.rs` (the extractor + ctx), and `backend/src/models/mod.rs::StepOutputFormat` for the Agent-side variants.

### Workflow Engine 0.7.x features (shipped in 0.6.0)

These features are tagged `0.7.0 Phase X` in the source but ship as part of release 0.6.0. They extend the workflow engine beyond pure agent-step pipelines.

**1. Guards — execution-bound limits.** `WorkflowGuards { timeout_seconds, max_llm_calls, loop_detection_max_revisits }` on `Workflow`. The runner aborts with a dedicated terminal status `RunStatus::StoppedByGuard` (rendered orange in the UI, distinct from `Failed` / `Cancelled`). The `loop_detection_max_revisits` field is a per-step counter consumed by `ConditionAction::Goto`, defaulting to 10. Migration `039_workflow_guards.sql`. Models in `backend/src/models/mod.rs` (search `WorkflowGuards`, `StoppedByGuard`).

**2. Artifacts — persisted step outputs.** `Workflow.artifacts: HashMap<String, ArtifactSpec>` declares typed artifacts. Agents emit `---ARTIFACT:name---...---END_ARTIFACT---` blocks; the runner extracts them, validates against `ArtifactSpec`, and writes them to disk in the run's workspace. Migration `040_workflow_artifacts.sql`. Implementation in `backend/src/workflows/steps.rs` (extraction) and `backend/src/workflows/runner.rs` (persistence).

**3. Gate — human-in-the-loop step.** `StepType::Gate` pauses the run with `RunStatus::WaitingApproval`. Workspace + git worktree are preserved during the pause. `POST /api/workflows/:id/runs/:run_id/decide` accepts `{ decision: "approve" | "request_changes" | "reject", comment? }` (`backend/src/api/workflows.rs::decide_run`). Optional `WorkflowStep.gate_notify_url` fires a best-effort webhook on entering pause (POST). Implementation: `backend/src/workflows/gate_step.rs`. Frontend RunDetail surfaces a "À VALIDER" badge + "en pause depuis Xh" counter on `WaitingApproval` runs.

**4. Exec — direct shell binary, zero tokens.** `StepType::Exec` runs a binary listed in `Workflow.exec_allowlist: Vec<String>` (exact-match allowlist; empty list = Exec disabled). Security invariants: never `sh -c`, args passed as separate argv literals (no shell interpolation), allowlist matched on the binary basename. Migration `043_workflow_exec_allowlist.sql`. Implementation: `backend/src/workflows/exec_step.rs` (header comment documents the threat model).

**5. Loops + run state.** `ConditionAction::Goto { step_name, max_iterations }` enables backward jumps from `on_result` rules. `WorkflowRun.state: HashMap<String,String>` (migration `042_workflow_run_state.sql`) is the durable scratchpad: agents write `---STATE:k=v---` lines, runner persists on the run row. Templates: `{{iter.<step_name>}}` (per-step revisit counter from the loop guard) and `{{state.<key>}}`. The `loop_detection_max_revisits` guard fires `StoppedByGuard` when the per-step iter exceeds the limit. Template plumbing in `backend/src/workflows/template.rs`.

**6. Rollback / `on_failure`.** `Workflow.on_failure: Vec<WorkflowStep>` — a separate step list that fires **only** when the run terminates with `RunStatus::Failed`. Skipped on `Cancelled`, `StoppedByGuard`, and Gate `reject`. Templates `{{failed_step.name}}` and `{{failed_step.output}}` are exposed inside on_failure prompts. Migration `041_workflow_on_failure.sql`. The wizard rejects `Gate` inside the rollback list (Notify + Agent + ApiCall accepted).

**7. Per-item Export / Import (Workflow + Quick Prompt).** Self-contained envelope JSON with `kind` (`"kronn-workflow"` or `"kronn-quick-prompt"`), `version`, `exported_at`. Endpoints:
- `GET /api/workflows/:id/export` + `POST /api/workflows/import` (`backend/src/api/workflows.rs::export_workflow` / `import_workflow`)
- `GET /api/quick-prompts/:id/export` + `POST /api/quick-prompts/import` (`backend/src/api/quick_prompts.rs::export_qp` / `import_qp`)

Workflow export bundles all Quick Prompts referenced by `BatchQuickPrompt` steps in the same envelope so a workflow round-trips standalone. Import strips per-user fields (e.g. `gate_notify_url`) so a shared workflow doesn't leak the previous owner's webhook URLs.

**8. Launch variables (manual trigger forms).** `Workflow.variables: Vec<PromptVariable>` (migration `044_workflow_variables.sql`) mirrors `QuickPrompt.variables`. When a user clicks "Lancer" on a Manual-trigger workflow with non-empty `variables`, the page shows a launch modal asking for one value per declared variable; values are merged into the run's `trigger_context` so `{{var_name}}` resolves in step prompts (existing `inject_trigger_context` handles the path). `POST /api/workflows/:id/trigger` accepts an optional `Json<TriggerWorkflowRequest>` body with `{ variables: HashMap<String,String> }`; missing/empty required variables produce an SSE error before the run starts. Legacy callers (no body) keep working — `Option<Json<...>>` → `None` → no variables. Frontend scanner (`frontend/src/lib/scanUndeclaredVars.ts`) runs live in the wizard and warns when a step prompt references a `{{var}}` that doesn't match any earlier step / state / iter / artifact / declared variable, with a 1-click "add to launch variables" affordance.

**Wizard 0.6.0 additions** (`frontend/src/lib/workflow-templates/v07-presets.ts`):
- 3 non-trivial presets — `AUTO_DEV` (auto-dev with tests), `PR_GATE` (PR creation with Gate approval), `DEPLOY_ROLLBACK` (deploy with on_failure rollback).
- STATE pedagogy chips on Agent step cards when an `on_result: Goto` is detected (hint at `{{state.X}}` write/read pattern).
- Rollback wizard accepts Notify + Agent + ApiCall, explicitly rejects Gate.

### Ticket Autopilot — preset 0.7+ (Sprint 1)

`TICKET_TO_PR` dans `frontend/src/lib/workflow-templates/v07-presets.ts` — workflow opinionated qui prend un ticket en entrée et drive jusqu'à la PR prête au merge. 9 steps + on_failure rollback :

```
fetch_issue (JsonData fixture, swappable→ApiCall)
  → analyze (Agent + writing-plans + brainstorming + verification)
  → plan_gate (humain valide le plan)
  → implement (Agent + tdd + debugging + verification + receiving-code-review)
  → run_tests (Exec, Goto implement on ERROR max 5)
  → review (Agent + requesting-code-review + verification, Goto implement on NEEDS_CHANGES)
  → create_pr (Agent + finishing-a-development-branch + verification)
  → ready_gate (humain valide la PR)
  → notify_done (Notify webhook)
```

Limites assumées Sprint 1 : pas de `Wait`/Poll CI auto (Sprint 3), pas de `skip_if` ni "Ask Human" mid-run dynamique (Sprint 2), pas de webhook receiver pour reprendre sur retours review humaine (Sprint 4), pas d'auto-merge ApiCall (v2).

Doc utilisateur complète : [`docs/ticket-autopilot.md`](../docs/ticket-autopilot.md).

### Vendored external skills (0.7+)

`backend/src/skills/external/` contient 8 skills méthodologiques **vendored** depuis [`obra/superpowers`](https://github.com/obra/superpowers) (MIT, commit `e7a2d164`, imported 2026-05-04) :

| Skill | Use case |
|-------|----------|
| `test-driven-development` | rituel red-green-refactor strict |
| `systematic-debugging` | root-cause à 4 phases |
| `writing-plans` | structurer un plan multi-step |
| `brainstorming` | explorer intent + design avant code |
| `verification-before-completion` | anti "done = compiled" — evidence avant claim |
| `requesting-code-review` | structurer une review |
| `receiving-code-review` | technique pour appliquer feedback review |
| `finishing-a-development-branch` | créer PR avec verify-tests-first |

Toutes auto-loadées par `BUILTIN_SKILLS` dans `core/skills.rs`. Frontmatter étendu avec `external: true`, `source_url`, `source_path`, `source_commit`, `imported_at`. Le pattern d'attribution (Caveman directive) est appliqué : description suffix `Adapted from <url> (<license>).` séparé visuellement (italique + opacité 70%) dans la skill card. Lien `🔗 Source` cliquable + badge `🔗 External` rendus par `SettingsPage::AttributedDescription`.

Pas de migration SQL : les skills sont embeddées au compile-time via `include_str!()`. Bump backend version → restart pour les voir.

Catalog des sources + licences : [`THIRD_PARTY_SKILLS.md`](../THIRD_PARTY_SKILLS.md). Update process documenté.

### Modularité unitaire des workflows (0.7+)

Trois briques pour factoriser les workflows et coller au use-case "remplacer un step mécanique par un appel direct" :

**1. `quick_api_id` étendu à `StepType::ApiCall` single** (était 0.6.0 pour `BatchApiCall` uniquement). Quand set, le runner charge le `QuickApi` référencé via `quick_api_hydrate::hydrate_step_from_quick_api` et hydrate les fields `api_*` manquants (per-field override : le step gagne quand non-vide). Pattern : factoriser un appel canonique en QA, le réutiliser dans N workflows. Wizard `ApiCallStepCard` expose un picker "Depuis un Quick API existant" + bandeau "🔗 Hérité de {QA}".

**2. `quick_prompt_id` sur `StepType::Agent`** — symétrique. Helper `quick_prompt_hydrate::hydrate_step_from_quick_prompt` injecte `prompt_template`, `tier`, et `skill_ids` du QP au run-time si le step les a vides. Pas de variables au niveau step : les `{{var}}` du QP sont résolus avec le `TemplateContext` du workflow (launch variables / state / previous_step / steps.X). Wizard expose un picker dans la step Agent card.

**3. `StepType::JsonData`** (`json_data_step.rs`) — source de données déterministe. Émet le payload littéral stocké dans `json_data_payload` sous forme d'envelope Structured. Zéro token, zéro réseau. Use case canonique : alimenter un `BatchQuickPrompt`/`BatchApiCall` sur une liste figée (10 hosts hardcodés, 5 régions, 3 environnements) sans monter d'API. Aussi : fixture de dev — on construit le pipeline sur du JsonData puis on remplace par un `ApiCall` quand la vraie source est prête. Validation au save : payload JSON non-null + ≤ 1 MiB. Wizard rend un textarea JSON avec parser live + counter d'items.

**Wizard preset bonus** : `daily-host-audit` démontre la combo `JsonData → BatchQuickPrompt → Notify` (5 hosts pré-câblés, l'utilisateur édite la liste + picke le QP audit).

Pas de migration SQL : steps en JSON dans `workflows.steps_json`, les nouveaux champs sont `Option<...>` avec `serde(default)`, donc compatibilité ascendante intacte.

### Desktop app (Tauri)

- **System tray**: closing the window hides to tray, backend + scheduler keep running. Tray menu: "Ouvrir Kronn" / "Quitter". Double-click to reopen.
- **Wake lock**: when cron workflows are active, prevents OS sleep (Windows: `SetThreadExecutionState`, macOS: `caffeinate -w`). Auto-releases when no cron workflows remain.
- **PATH enrichment**: GUI apps on macOS inherit minimal PATH. `enrich_path()` at startup adds homebrew, npm global, cargo, nvm, fnm, bun, uv directories if they exist.
- **Desktop signing and verification**: Tauri applies an ad-hoc macOS identity
  before assembling the DMG unless CI supplies a Developer ID. The release job
  mounts the produced image and verifies the contained app with strict
  `codesign`; ad-hoc builds may still require explicit Gatekeeper approval or
  `xattr -cr /Applications/Kronn.app`. Windows loads WeasyPrint's Pango DLLs
  from MSYS2 UCRT64 and performs an early sidecar smoke build.
- **Desktop backend ownership**: the desktop process acquires the shared data
  directory lock before booting and always starts its own backend on a free
  loopback port. It never reuses another CLI/dev/Docker listener, even at the
  same version, because its origin policy and runtime state cannot be assumed
  compatible. A lock conflict or startup failure is rendered by the bundled
  frontend with an actionable restart button; optional Docs-sidecar resource
  resolution degrades without terminating the desktop shell. Tauri's ACL gives
  the bundled bootstrap only `wait_for_backend` and `restart_app`; after the
  verified loopback navigation, that origin retains only `restart_app`.
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
- Migration 090 stores `mcp_configs.preferred_interface` (`api | mcp | cli`).
  The Plugins drawer derives the selectable modes from the server's actual
  capabilities, and the update endpoint rejects unavailable modes. For each
  active multi-interface plugin,
  `build_plugin_invocation_preferences()` adds one compact preference rule to
  the shared MCP context; single-interface plugins emit nothing. The same
  context reaches Claude, Codex, Vibe, Gemini, Kiro, Copilot and Ollama.
- `sync_project_mcps_to_disk()` matches on `transport` — `ApiOnly` is a silent skip.
- The `collect_active_api_plugins()` helper fetches + decrypts active API configs per project.
- `config_keys` lets a plugin declare non-secret parameters (e.g. Chartbeat's `host`, Adobe's `company_id`). The UI renders them as plain inputs with the provided `label` + `placeholder` + `description`; the prompt injection surfaces them alongside the auth so the agent has enough to build a full URL.
- **Readiness probes are proofs, not presence checks.** Built-in APIs opt into
  an explicitly side-effect-free GET through
  `registry::api_readiness_probe`; it runs through the production API executor
  with the configured credential, but its response body is discarded and not
  persisted in API-call logs. Stdio MCPs must answer a bounded JSON-RPC
  `initialize`; SSE and Streamable HTTP transports must negotiate their real
  endpoint. A custom API with no trusted probe declaration stays non-ready
  rather than receiving a fake green state.
- `{ENV_KEY}` templating works in `ApiSpec.base_url` AND `OAuth2ExtraHeader.value_template`. Missing keys render as `<NOT_CONFIGURED:KEY>`.
- Default context (`default_context` on the registry entry) is auto-written to `docs/operations/mcp-servers/<slug>.md` at install time — for API plugins too.

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
- ~~New workflow step type `ApiCall`~~ — **shipped** in 0.5.1, extended to `quick_api_id` reference in 0.7+ (see "Modularité unitaire des workflows" above).
- Next OAuth2 plugins candidates: Google Analytics 4 Data API, Salesforce REST — same `OAuth2ClientCredentials` variant, different `token_url` + scopes + extra headers.

### Host MCP sync — bidirectional CLI integration (0.6.0 in development)

Kronn-managed MCPs are now also written into the user's local CLI config files (`~/.claude.json`, `~/.gemini/settings.json`, `~/.codex/config.toml`, `~/.copilot/mcp-config.json`) so that running `claude`, `gemini`, `codex`, or `copilot` **outside** Kronn surfaces the same MCPs. Three-phase architecture; both inbound (read-only audit) and outbound (write) flows.

**Phase 1 — Inbound discovery** (`backend/src/core/host_mcp_discovery.rs`): scans the 4 host config files and surfaces every MCP entry found, with ownership classification (`NotManaged` / `ManagedByMarker(config_id)` / `ManagedByHash(config_id)`). Read-only — never mutates disk. Endpoint `GET /api/mcps/host-discovery`. Frontend section in Settings → "MCPs externes détectés".

**Phase 2 — Adopt** (`POST /api/mcps/host-discovery/adopt`): converts a non-Kronn-managed entry into a Kronn `McpConfig`. Defaults to `host_sync = GlobalOnly` + `is_global = false` + `project_ids = []`. Source classified as `McpSource::HostImported` (separate from `Detected` which is project-`.mcp.json`-scoped). Idempotent (hash dedup). Source file is **never modified** during adopt — that's the user's existing config; Kronn just registers a parallel record.

**Phase 3 — Outbound sync** (4 functions in `backend/src/core/mcp_scanner.rs`):
| Function | Target | Routing |
|----------|--------|---------|
| `sync_codex_global_config` | `~/.codex/config.toml` `[mcp_servers.*]` | top-level only (Codex has no native per-project scope) |
| `sync_copilot_global_config` | `~/.copilot/mcp-config.json` | top-level only |
| `sync_gemini_global_config` | `~/.gemini/settings.json` `mcpServers` | top-level only — Gemini's `httpUrl` used for Streamable HTTP |
| `sync_claude_global_config` | `~/.claude.json` | **scope-aware**: `is_global=true` → top-level `mcpServers`; `is_global=false + project_ids` → `projects[<host-path>].mcpServers` for each project; `project_ids=[]` → top-level fallback |

All four filter `host_sync ∈ {GlobalOnly, MirrorAll}` (= "synced to host"), skip `ApiOnly` transport, and use the same defensive merge pattern: `_kronn` marker on each managed entry (`{managed: true, config_id: <uuid>}`), tree-wide cleanup of orphaned Kronn entries, atomic write (`tmp+rename`), `chmod 0600` on Unix, abort+`.kronn-backup` on parse failure (data preservation).

**Data model (`backend/src/models/mod.rs`)**:
- `HostSyncMode` enum: `None | GlobalOnly | MirrorAll` — column on `mcp_configs` (migration 036). `MirrorAll` is a legacy value the codebase still reads but never writes (collapsed to `GlobalOnly` by migration 038).
- `McpSource::HostImported` — new variant for entries adopted from host files (vs `Registry` / `Detected` / `Manual`).

**Migrations**:
- `036_mcp_host_sync.sql` — adds `host_sync TEXT NOT NULL DEFAULT 'None'` to `mcp_configs`.
- `037_mcp_host_sync_backfill.sql` — sets `host_sync = 'MirrorAll'` on every existing config so the upgrade preserves the pre-0.6.0 behavior (Codex/Copilot were already global-by-default).
- `038_mcp_host_sync_collapse.sql` — converts `MirrorAll` rows into the orthogonal pair `is_global = 1 + host_sync = 'GlobalOnly'`. The 3-mode UI was found to be confusing (override of project_ids), so the column is now binary in practice and `is_global` carries the "applied to all Kronn projects" semantics alone.

**UX (Plugins page is the canonical editor — see `feedback_mcp_single_edit.md` memory)**:
- `HostSyncChip` (`frontend/src/components/HostSyncChip.tsx`) — `🌐 CLI local` badge on every MCP card whose `host_sync !== 'None'`. At-a-glance scan of the fleet.
- Drawer `Scope` section: project toggles + `is_global` toggle (existing) + checkbox "Aussi disponible dans mes CLIs locaux" (new).
- `HostSyncPreview` (`frontend/src/components/HostSyncPreview.tsx`) — dynamic preview under the checkbox listing the exact destination per CLI based on current scope (e.g. `Claude Code → ~/.claude.json › projects[/home/me/Repos/APP_ANDROID]`, `Gemini → ~/.gemini/settings.json (top-level — scope projet non supporté)`). Renders the asymmetry (Claude scope-aware vs others top-level only) before the user clicks Save.
- Empty-state banner on Plugins page when no config has `host_sync !== 'None'` (one-click jump to first config). Coach mark "✨ Nouveau" on the checkbox section, one-shot localStorage flag.

**Host-sync resilience** (resolved 0.6.0 / 2026-05-07 — kept here as
historical context for code reviewers):
- Trait abstraction: `HostMcpSync` + `run_host_sync` driver.
  Adding a 5th CLI = 1 struct + 1 entry in the registry slice.
- Concurrent-write guard: `atomic_write_checked` snapshots mtime
  before read and aborts the rename if a third party (Claude Code,
  Gemini CLI, …) bumped it.
- Workflow-run gate: `db::workflows::has_running_run` short-circuits
  `sync_affected_projects` when an agent is mid-spawn.
- Backup rotation: `rotate_backup(path, 5)` keeps `.1` (newest) →
  `.5` (oldest) on each parse-fail; oldest dropped automatically.

### Document generation — Kronn Docs (0.5.1)

Agents produce 5 file formats (PDF / DOCX / XLSX / CSV / PPTX) without the user installing anything.

**Sidecar architecture** (`backend/sidecars/docs/`): Python FastAPI + uvicorn on a random loopback port, started by backend during boot. Deterministic startup via `KRONN_DOCS_READY <port>` stdout marker. Dependencies: WeasyPrint (PDF), python-docx + BeautifulSoup (DOCX, HTML→Word mapping), XlsxWriter (XLSX), stdlib `csv` (CSV), python-pptx (PPTX). Docker bakes the virtualenv into its image; desktop builds freeze a standalone executable with PyInstaller and include it as a Tauri resource. `make docs-setup` is only the native source-development fallback. Missing/corrupt release resources degrade to a user-facing update/reinstall error instead of a hard failure.

**Rust proxy** (`backend/src/api/docs.rs` + `backend/src/core/docs_sidecar.rs`): 5 endpoints `POST /api/docs/{pdf,docx,xlsx,csv,pptx}` + `GET /api/docs/file/:discussion_id/:filename`. All five POST handlers funnel through a single `proxy_to_sidecar()` helper — adding a format = one arm. Output files land in `~/.kronn/generated/<discussion_id>/`. Filename sanitization (alphanumerics + `-_ ` only, UUID suffix, extension forced) + canonicalize check in `download_file` guard against path traversal.

**Agent contract** — skill `kronn-docs.md` ships two fence conventions:
- ```` ```kronn-doc-preview ```` — HTML body used for PDF + DOCX. Frontend renders a sandboxed iframe (`sandbox=""`) + two export buttons.
- ```` ```kronn-doc-data ```` — JSON payload `{format, ...}` for structured formats (XLSX / CSV / PPTX). No preview; compact card with summary (row count, sheet count, slide count) + single export button.

Auto-activation: the skill carries `auto_triggers.common/fr/en/es` regex buckets — "génère un rapport PDF", "create a presentation", "exporta hoja xlsx" etc. Matched skills auto-inject into the system prompt. Per-skill opt-out via Settings (`auto_triggers` table).

**Frontend** — `frontend/src/components/DocPreview.tsx` (HTML formats) + `DocDataExport.tsx` (structured). Both detected in `MarkdownContent` (`MessageBubble.tsx`) by the fence's `language-kronn-doc-*` class. Malformed JSON / unknown format discriminator falls back to a normal `<pre>` so a bad agent message can't blow up the chat.

---

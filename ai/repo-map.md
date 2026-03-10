# Repository map (where things live)

## What this document is
- Focus: *where* code/config/tests live.
- For *how to run checks*: see `ai/testing-quality.md`.

## Stack overview (facts)

Rust backend (axum) + React/TypeScript frontend (Vite) + nginx gateway, all in Docker Compose.

## Key folders (facts)

```
Kronn/
├── backend/                    # Rust backend (axum web server)
│   ├── Cargo.toml              # Dependencies: axum 0.7, tokio, serde, ts-rs, anyhow
│   └── src/
│       ├── main.rs             # Entrypoint, router definition, AppState
│       ├── models/mod.rs       # All data models (Project, Discussion, MCP, Workflow, Config...)
│       ├── api/                # HTTP handlers (one file per domain)
│       │   ├── mod.rs          # Re-exports
│       │   ├── setup.rs        # Setup wizard + config endpoints (tokens, language, agents)
│       │   ├── projects.rs     # Project CRUD + scan + AI audit pipeline (template install, SSE audit, validation)
│       │   ├── discussions.rs  # Discussion CRUD + SSE streaming + orchestration
│       │   ├── mcps.rs         # MCP 3-tier API: overview, configs CRUD, registry, refresh, secrets
│       │   ├── tasks.rs        # Legacy scheduled tasks (being replaced by workflows)
│       │   ├── workflows.rs    # [planned] Workflow CRUD + trigger + runs
│       │   ├── agents.rs       # Agent detection + install + uninstall + toggle (enable/disable)
│       │   └── stats.rs        # Token usage stats
│       ├── agents/             # Agent runner (CLI execution)
│       │   ├── mod.rs          # Re-exports
│       │   └── runner.rs       # Spawns agent CLIs, streams stdout as SSE. Two output modes: Text (line-by-line) and StreamJson (Claude Code stream-json with token tracking). Runtime probe (npx fallback, 5min cache). MCP contexts injected into prompts
│       ├── db/                 # SQLite persistence layer
│       │   ├── mod.rs          # Database struct (Mutex<Connection>), with_conn() async accessor, init
│       │   ├── migrations.rs   # Versioned migration runner (run before Mutex wrap)
│       │   ├── projects.rs     # Project CRUD operations
│       │   ├── discussions.rs  # Discussion + message CRUD operations
│       │   ├── mcps.rs         # MCP servers/configs/linkages CRUD, encryption, hashing
│       │   ├── workflows.rs    # Workflow + WorkflowRun CRUD, run deletion (individual + bulk)
│       │   └── sql/
│       │       ├── 001_initial.sql      # Schema: projects, tasks, discussions, messages
│       │       ├── 002_mcp_redesign.sql # 3-tier MCP: mcp_servers, mcp_configs, mcp_config_projects
│       │       └── 004_token_tracking.sql # Token tracking tables
│       ├── core/               # Business logic
│       │   ├── mod.rs          # Re-exports
│       │   ├── config.rs       # Config load/save (~/.config/kronn/)
│       │   ├── scanner.rs      # Git repo scanner + AI audit detection (detect_audit_status, count_ai_todos)
│       │   ├── registry.rs     # MCP registry (19 built-in official servers, grouped by category, with token_url/token_help)
│       │   ├── mcp_scanner.rs  # Multi-agent MCP sync + MCP injection. read_all_mcp_contexts() reads .mcp.json + context files and generates prompt listing available MCP tools. Disk sync: .mcp.json (Claude), .vibe/config.toml (Vibe), ~/.codex/config.toml (Codex). .gitignore safety
│       │   └── crypto.rs       # AES-256-GCM encryption for MCP secrets
│       ├── scheduler/mod.rs    # Legacy cron-based task scheduler (to be replaced)
│       └── workflows/          # Workflow engine (implemented)
│           ├── mod.rs          # WorkflowEngine: background polling loop (30s ticks), trigger checking, concurrency
│           ├── trigger.rs      # Cron evaluation, tracker polling frequency
│           ├── runner.rs       # Orchestrates full run: workspace → hooks → steps → cleanup (no separate actions phase)
│           ├── steps.rs        # Step execution: prompt rendering, stall detection, retry, on_result conditions
│           ├── template.rs     # Liquid-compatible template engine for {{variable}} substitution
│           ├── workspace.rs    # Git worktree create/cleanup with lifecycle hooks
│           └── tracker/
│               ├── mod.rs      # TrackerSource trait (poll, update_status, comment, create_pr)
│               └── github.rs   # GitHub API v3 implementation (reqwest + rustls)
│
├── frontend/                   # React + TypeScript (Vite)
│   ├── package.json
│   ├── tsconfig.json           # ES2020, strict, react-jsx
│   ├── vite.config.ts
│   └── src/
│       ├── main.tsx            # React DOM entry
│       ├── App.tsx             # Router (setup wizard vs dashboard)
│       ├── pages/
│       │   ├── Dashboard.tsx   # Main UI shell (projects, discussions, workflows, settings) — routes to sub-pages
│       │   ├── McpPage.tsx     # MCP management (registry, configs, inline secret editing with per-field visibility, context files, project toggles)
│       │   ├── WorkflowsPage.tsx # Workflow management (list, 5-step create wizard, detail + live run progress via SSE, run deletion, manual trigger)
│       │   └── SetupWizard.tsx # First-run setup flow
│       ├── hooks/
│       │   └── useApi.ts       # Generic fetch hook with loading/error state
│       ├── lib/
│       │   ├── api.ts          # API client (typed wrappers + SSE streaming helpers)
│       │   ├── i18n.ts         # Lightweight i18n system (fr/en/es). Translation dictionaries + locale persistence (localStorage)
│       │   └── I18nContext.tsx  # React context provider for UI locale (useT() hook)
│       └── types/
│           └── generated.ts    # Auto-generated from Rust models (DO NOT EDIT)
│
├── ai/                         # AI context documentation (for this repo)
├── templates/                  # AI context templates (for projects managed by Kronn)
│   ├── CLAUDE.md
│   └── ai/                     # Template files
│
├── lib/                        # CLI shell libraries (Bash 3.2+ compatible — macOS, Linux, WSL)
│   ├── ui.sh                   # Terminal UI helpers (colors, prompts, banners). Interactive menu with fallback to numbered input on Bash < 4
│   ├── agents.sh               # Agent detection + install. Parallel arrays (no associative arrays for Bash 3.2 compat)
│   ├── mcps.sh                 # MCP sync + secrets
│   ├── repos.sh                # Repo scanning. Uses rsync/find+cp fallback instead of cp -rn (BSD compat)
│   ├── tron.sh                 # Tron-themed animated progress loader
│   └── analyze.sh              # AI config analysis. Detects GNU/BSD sed for sed -i compat
├── kronn                       # CLI entrypoint (bash script, cross-platform)
├── docker-compose.yml          # 3 services: backend, frontend, gateway
├── Makefile                    # start, stop, logs, build, dev-backend, dev-frontend, typegen
└── .docker/                    # Docker configs (nginx gateway)
```

## Primary entrypoints for conventions

- **Route registration**: `backend/src/main.rs` — all API routes defined here.
- **Data models**: `backend/src/models/mod.rs` — single file, source of truth for all types.
- **API client**: `frontend/src/lib/api.ts` — all fetch calls, SSE streaming logic.
- **Type generation**: `make typegen` reads `#[derive(TS)]` attributes in Rust models.
- **CLI commands**: `kronn` script, sources `lib/*.sh`.

## Notes
- `README.md` is not guaranteed to be up-to-date; prefer actual config files as source of truth.
- `frontend/src/types/generated.ts` is auto-generated — never edit manually.
- Dashboard.tsx (~1900 lines) is the main shell with projects, discussions, and settings pages. MCP page extracted to McpPage.tsx (~625 lines), Workflows to WorkflowsPage.tsx (~1600 lines, includes wizard + live progress + run management).
- `templates/` directory contains the AI context template files (ai/ skeleton, CLAUDE.md, .cursorrules, etc.) mounted at `/app/templates:ro` in Docker.

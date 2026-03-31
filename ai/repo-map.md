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
│       ├── main.rs             # Entrypoint, server startup, graceful shutdown (SIGTERM/SIGINT)
│       ├── lib.rs              # Router definition (build_router), auth middleware, CORS, AppState
│       ├── models/mod.rs       # All data models (Project, Discussion, MCP, Workflow, Config...)
│       ├── api/                # HTTP handlers (one file per domain)
│       │   ├── mod.rs          # Re-exports
│       │   ├── setup.rs        # Setup wizard + config endpoints (tokens, language, agents, server config, auth token)
│       │   ├── projects.rs     # Project CRUD (~1396L) + scan + bootstrap + clone + template install + git ops + defaults
│       │   ├── audit.rs        # AI audit pipeline (~1848L) — SSE audit, full_audit, drift, validation, briefing, cancel, skill detection
│       │   ├── ai_docs.rs      # AI doc file browser (~184L) — list/search/read ai/ files
│       │   ├── discover.rs     # Remote repo discovery (~426L) — GitHub/GitLab multi-source with token from MCPs
│       │   ├── discussions.rs  # Discussion CRUD + SSE streaming + orchestration (~3696L)
│       │   ├── contacts.rs     # Contacts CRUD + invite codes + network info + ping
│       │   ├── ws.rs           # WebSocket handler — peer-to-peer presence + auto-add unknown peers
│       │   ├── mcps.rs         # MCP 3-tier API: overview, configs CRUD, registry, refresh, secrets
│       │   ├── workflows.rs    # Workflow CRUD + trigger + runs
│       │   ├── agents.rs       # Agent detection + install + uninstall + toggle (enable/disable)
│       │   ├── stats.rs        # Token usage & cost stats (by provider, project, daily history, top discussions/workflows)
│       │   ├── skills.rs       # Skills API: list, create, update, delete
│       │   ├── profiles.rs     # Profiles API: list, create, update, delete, persona-name override
│       │   ├── directives.rs   # Directives API: list, create, update, delete
│       │   └── git_ops.rs      # Shared git helpers (838L) — used by projects + discussions
│       ├── agents/             # Agent runner (CLI execution)
│       │   ├── mod.rs          # Agent detection: PATH → KRONN_HOST_BIN (with .cmd/.exe extension matching) → WSL (via bash -lc). Version detection handles WSL paths. Runtime probe (npx fallback, 5min cache). 6 agents: Claude, Codex, Vibe, Gemini, Kiro, Copilot
│       │   └── runner.rs       # Spawns agent CLIs, streams stdout as SSE. Two output modes: Text (line-by-line) and StreamJson (Claude Code stream-json with token tracking). Cross-platform HOME resolution (KRONN_HOST_HOME → HOME → USERPROFILE). COPILOT_HOME for Copilot CLI auth. MCP contexts injected into prompts
│       ├── db/                 # SQLite persistence layer
│       │   ├── mod.rs          # Database struct (Mutex<Connection>), with_conn() async accessor, init
│       │   ├── migrations.rs   # Versioned migration runner (run before Mutex wrap)
│       │   ├── projects.rs     # Project CRUD operations
│       │   ├── discussions.rs  # Discussion + message CRUD (+ archive/rename via update_discussion)
│       │   ├── discussions_test.rs # 21 tests (CRUD, archive, title, messages, AgentType round-trip for all 6 agents + Custom, DB string stability)
│       │   ├── mcps.rs         # MCP servers/configs/linkages CRUD, encryption, hashing
│       │   ├── workflows.rs    # Workflow + WorkflowRun CRUD, run deletion (individual + bulk)
│       │   └── sql/
│       │       ├── 001_initial.sql      # Schema: projects, discussions, messages (+ legacy tasks table)
│       │       ├── 002_mcp_redesign.sql # 3-tier MCP: mcp_servers, mcp_configs, mcp_config_projects
│       │       ├── 004_token_tracking.sql # Token tracking tables
│       │       ├── 005_discussion_archive.sql # Add archived column to discussions
│       │       ├── 006_discussion_skills.sql # Add skill_ids_json column to discussions
│       │       ├── 007_project_skills.sql   # Default skills per project
│       │       ├── 008_discussions_index.sql # Performance index
│       │       ├── 009_profiles.sql          # Profile support (profile_id on discussions/projects)
│       │       ├── 010_directives.sql        # Directives support
│       │       ├── 011_multi_profiles.sql    # Multi-profile support (profile_id → profile_ids_json)
│       │       ├── 012_mcp_general.sql       # Global MCP configs
│       │       ├── 013_discussion_worktrees.sql # Worktree support for discussions
│       │       ├── 014_summary_cache.sql     # Summary caching
│       │       ├── 015_model_tier.sql        # ModelTier system
│       │       ├── 016_message_model_tier.sql # Per-message model tier
│       │       ├── 017_message_count.sql     # Message count tracking
│       │       ├── 018_briefing_notes.sql    # Pre-audit briefing notes
│       │       ├── 019_pin_first_message.sql # Pin first message feature
│       │       ├── 020_fix_worktree_paths.sql # Fix worktree relative paths
│       │       ├── 021_message_identity.sql # Author pseudo + avatar on messages
│       │       ├── 022_contacts.sql         # Contacts table (multi-user)
│       │       ├── 023_shared_discussions.sql # Shared discussions (multi-user)
│       │       └── 024_message_cost.sql     # Per-message cost_usd column
│       ├── core/               # Business logic
│       │   ├── mod.rs          # Re-exports
│       │   ├── config.rs       # Config load/save (~/.config/kronn/)
│       │   ├── scanner.rs      # Git repo scanner + AI audit detection (detect_audit_status, count_ai_todos). WSL UNC paths (\\wsl.localhost\...) run git via wsl.exe
│       │   ├── registry.rs     # MCP registry (48 built-in official servers, grouped by category, with token_url/token_help)
│       │   ├── mcp_scanner.rs  # Multi-agent MCP sync + MCP injection. read_all_mcp_contexts() reads .mcp.json + context files. Disk sync: .mcp.json (Claude), .vibe/config.toml (Vibe), ~/.codex/config.toml (Codex). .gitignore safety
│       │   ├── native_files.rs # Native SKILL.md + agent file sync. Writes skills to .claude/skills/, .agents/skills/, .gemini/skills/. Profiles to .claude/agents/, .gemini/agents/, .codex/agents/. Additive sync for discussions, full cleanup at startup.
│       │   ├── tailscale.rs   # Network & VPN auto-detection (Tailscale, VPN, LAN IPs). KRONN_HOST_IPS env for Docker. Used for multi-user invite codes.
│       │   ├── ws_client.rs   # WebSocket client manager: outbound connections to contacts with exponential backoff. Auto-reconnects.
│       │   ├── crypto.rs       # AES-256-GCM encryption for MCP secrets
│       │   ├── skills.rs      # Skills loader: builtin (embedded .md) + custom (~/.config/kronn/skills/). Frontmatter parsing, build_skills_prompt()
│       │   ├── profiles.rs   # Profiles loader: builtin (embedded .md) + custom (~/.config/kronn/profiles/). Persona override system, build_profiles_prompt()
│       │   ├── directives.rs # Directives loader: builtin (embedded .md) + custom (~/.config/kronn/directives/). build_directives_prompt()
│       │   ├── cmd.rs        # Cross-platform command helpers: async_cmd()/sync_cmd() apply CREATE_NO_WINDOW on Windows. ALL Command::new() calls MUST use these helpers
│       │   └── pricing.rs    # Static token pricing table (per-provider $/1M tokens). estimate_cost() fallback when real cost unavailable
│       ├── profiles/          # Builtin profile Markdown files (8 profiles: architect, tech-lead, qa-engineer, product-owner, scrum-master, technical-writer, devils-advocate, mentor)
│       ├── directives/        # Builtin directive Markdown files
│       ├── skills/             # Builtin skill Markdown files (embedded at compile time)
│       │   ├── token-saver.md  # Meta: minimize token usage
│       │   ├── typescript-dev.md # Technical: TypeScript expert
│       │   ├── rust-dev.md     # Technical: Rust expert
│       │   ├── security-auditor.md # Technical: security review
│       │   ├── product-owner.md # Business: user perspective
│       │   ├── devils-advocate.md # Meta: challenge assumptions
│       │   ├── qa-engineer.md  # Business: testing focus
│       │   ├── devops-expert.md # Technical: infrastructure & ops
│       │   ├── seo-expert.md   # Technical: SEO optimization
│       │   ├── green-it-expert.md # Technical: environmental efficiency
│       │   ├── data-engineer.md # Technical: data pipelines
│       │   ├── tech-lead.md    # Technical: architecture & leadership
│       │   └── json-output.md  # Meta: force JSON-only output
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
│   ├── package.json            # engines: node>=24 (LTS)
│   ├── tsconfig.json           # ES2020, strict, react-jsx
│   ├── vite.config.ts          # Build config + test config (vitest) + code splitting
│   ├── eslint.config.js        # ESLint 10 flat config (typescript-eslint strict)
│   └── src/
│       ├── main.tsx            # React DOM entry
│       ├── App.tsx             # Router (setup wizard vs dashboard) + ErrorBoundary + React.lazy code splitting
│       ├── pages/
│       │   ├── Dashboard.tsx   # Main UI shell (~674L) — nav bar, page routing, shared state. Project list extracted to components/ProjectList + ProjectCard
│       │   ├── SettingsPage.tsx # Settings (~1870 lines) — language, voice (TTS/STT model selection), agents config, tokens, usage stats, DB management
│       │   ├── DiscussionsPage.tsx # Discussions orchestrator (~1218L) — state, streaming, callbacks. Split into components below.
│       │   ├── McpPage.tsx     # MCP management (registry, configs, inline secret editing with per-field visibility, context files, project toggles)
│       │   ├── WorkflowsPage.tsx # Workflow management (~1780L, list, wizard, detail, runs, access warnings)
│       │   ├── SetupWizard.tsx # First-run setup flow
│       │   └── *.css           # Per-page CSS files (tokens + utilities in src/styles/)
│       ├── components/
│       │   ├── ChatHeader.tsx    # Discussion chat header (502L) — title editing, agent badges, MCP/settings popovers, git toggle
│       │   ├── ChatInput.tsx     # Chat input composer (695L) — textarea, @mentions, voice STT, debate popover, send/stop
│       │   ├── DiscussionSidebar.tsx # Sidebar (346L) — discussion list, contacts, search, archives
│       │   ├── NewDiscussionForm.tsx  # New discussion form (447L) — project/agent/skills/profiles/directives selection
│       │   ├── MessageBubble.tsx # Message bubble (329L) — user/agent/system, markdown, TTS, edit, copy, retry
│       │   ├── SwipeableDiscItem.tsx  # Swipeable sidebar item (110L) — swipe-to-archive/delete
│       │   ├── GitPanel.tsx      # Git file/branch panel
│       │   ├── AiDocViewer.tsx   # AI doc viewer
│       │   ├── ProjectList.tsx   # Project list with search, filter, group-by-org (234L)
│       │   ├── ProjectCard.tsx   # Single project accordion card — discussions, AI docs, MCPs, workflows, skills, audit (707L)
│       │   └── settings/
│       │       ├── AgentsSection.tsx  # Agent config (tokens, keys, model tiers, install/uninstall)
│       │       ├── UsageSection.tsx   # Usage dashboard (summary cards, provider bar, project bars, daily chart, top-5 lists, tokens/cost toggle, disc/wf filter)
│       │       ├── IdentitySection.tsx # User identity (pseudo, avatar, invite code)
│       │       └── ProfilesSection.tsx # Profile management
│       ├── styles/
│       │   ├── tokens.css        # CSS custom properties (--kr-bg-*, --kr-text-*, --kr-accent-*, --kr-sp-*, --kr-r-*, --kr-fs-*)
│       │   ├── reset.css         # Global reset + font-face (moved from index.html)
│       │   ├── utilities.css     # Utility classes (.flex-row, .gap-*, .text-*, .rounded-*, .mb-*, etc.)
│       │   ├── components.css    # Shared component classes (.btn, .card, .input, .badge, .dot, .code, .label)
│       │   └── index.css         # Barrel import for all CSS files
│       ├── hooks/
│       │   ├── useApi.ts       # Generic fetch hook with loading/error/refetch + race condition protection
│       │   └── useToast.ts     # Toast notifications (success/error/info, auto-dismiss 4s, max 3 visible)
│       ├── lib/
│       │   ├── api.ts          # API client (typed wrappers + SSE streaming helpers)
│       │   ├── i18n.ts         # Lightweight i18n system (fr/en/es). Translation dictionaries + locale persistence (localStorage)
│       │   ├── I18nContext.tsx  # React context provider for UI locale (useT() hook)
│       │   ├── constants.ts    # Shared constants: AGENT_COLORS, AGENT_LABELS, ALL_AGENT_TYPES, agentColor()
│       │   ├── tts-engine.ts   # TTS playback engine: pause/resume/stop, sentence pipelining, SpeechSynthesis fallback
│       │   ├── tts-utils.ts    # Markdown→speech text cleaning + sentence splitting
│       │   ├── tts-models.ts   # Piper voice definitions (9 voices, 3 langs) + localStorage persistence
│       │   ├── tts-worker.ts   # Web Worker: Piper WASM inference (@diffusionstudio/vits-web)
│       │   ├── stt-engine.ts   # STT recording: audio resampling 16kHz, Whisper worker communication
│       │   ├── stt-models.ts   # Whisper model definitions (tiny/base/small) + localStorage persistence
│       │   └── stt-worker.ts   # Web Worker: Whisper WASM inference (@huggingface/transformers)
│       ├── types/
│       │   └── generated.ts    # Auto-generated from Rust models (DO NOT EDIT)
│       ├── test/
│       │   └── setup.ts        # Test setup (@testing-library/jest-dom)
│       ├── __tests__/          # App-level tests (App.tsx, ErrorBoundary)
│       ├── hooks/__tests__/    # Hook tests (useApi)
│       ├── lib/__tests__/      # Lib tests (i18n, api, constants, types, regression, access-warnings)
│       └── pages/__tests__/    # Page component tests (WorkflowsPage, DiscussionsPage, SettingsPage, McpPage)
│
├── ai/                         # AI context documentation (for this repo)
├── templates/                  # AI context templates (for projects managed by Kronn)
│   ├── CLAUDE.md
│   └── ai/                     # Template files
│
├── lib/                        # CLI shell libraries (Bash 3.2+ compatible — macOS, Linux, WSL)
│   ├── ui.sh                   # Terminal UI helpers (colors, prompts, banners). Interactive menu with fallback to numbered input on Bash < 4
│   ├── agents.sh               # Agent detection + install (5 agents incl. Kiro). Parallel arrays (no associative arrays for Bash 3.2 compat)
│   ├── mcps.sh                 # MCP sync + secrets
│   ├── repos.sh                # Repo scanning. Uses rsync/find+cp fallback instead of cp -rn (BSD compat)
│   ├── tron.sh                 # Tron-themed animated progress loader
│   └── analyze.sh              # AI config analysis. Detects GNU/BSD sed for sed -i compat
├── tests/bats/                 # Shell tests (bats-core, 8 suites, 186 tests)
│   ├── run.sh                  # Test runner
│   ├── test_helper.bash        # Shared helper (_load_lib, color vars)
│   ├── agents.bats             # agents.sh tests (37)
│   ├── mcps.bats               # mcps.sh tests (19)
│   ├── tron.bats               # tron.sh tests (32)
│   ├── ui.bats                 # ui.sh tests (28)
│   ├── repos.bats              # repos.sh tests (24)
│   ├── analyze.bats            # analyze.sh tests (16)
│   ├── portability.bats        # Cross-platform tests (18)
│   └── bugfixes.bats           # Non-regression tests (12)
├── kronn                       # CLI entrypoint (bash script, cross-platform)
├── desktop/                    # Tauri desktop app (native Windows/macOS/Linux wrapper)
│   ├── package.json            # Desktop app dependencies
│   └── src-tauri/              # Tauri Rust backend (embedded server, COOP/COEP headers)
├── docker-compose.yml          # 3 services: backend, frontend, gateway
├── Makefile                    # start, stop, logs, build, dev-backend, dev-frontend, typegen
└── .docker/                    # Docker configs (nginx gateway)
```

## Primary entrypoints for conventions

- **Route registration**: `backend/src/lib.rs` (`build_router()`) — all API routes defined here.
- **Data models**: `backend/src/models/mod.rs` — single file, source of truth for all types.
- **API client**: `frontend/src/lib/api.ts` — all fetch calls, SSE streaming logic.
- **Type generation**: `make typegen` reads `#[derive(TS)]` attributes in Rust models.
- **CLI commands**: `kronn` script, sources `lib/*.sh`.

## Notes
- `README.md` is not guaranteed to be up-to-date; prefer actual config files as source of truth.
- `frontend/src/types/generated.ts` is auto-generated — never edit manually.
- Dashboard.tsx (~674L) is the main UI shell (nav bar, page routing). Sub-pages: SettingsPage (~990L + 3 sections), DiscussionsPage (~1241L + 6 components), McpPage (~740L), WorkflowsPage (~373L + 3 components). Projects: ProjectList (~234L) + ProjectCard (~707L).
- DiscussionsPage split (2026-03-28): ChatHeader, ChatInput, DiscussionSidebar, NewDiscussionForm, MessageBubble, SwipeableDiscItem — each < 700L.
- CSS system: `src/styles/` (tokens, utilities, components) + per-page CSS. ~319 inline styles remain (dynamic only).
- TTS/STT logic extracted into `lib/tts-*.ts` and `lib/stt-*.ts` modules (7 files, ~400 lines total). Web Workers for WASM inference run off the main thread.
- Shared constants (AGENT_COLORS, AGENT_LABELS) extracted to `lib/constants.ts` — imported by Dashboard and WorkflowsPage.
- Frontend tests in `__tests__/` directories alongside source (22 suites, ~315 tests). See `ai/testing-quality.md`.
- Shell tests in `tests/bats/` (8 suites, 186 tests via bats-core). See `ai/testing-quality.md`.
- CI pipeline: `.github/workflows/ci-test.yml` triggered on push to main + all PRs (backend clippy/test + frontend tsc/test + shell bats + security scan). Desktop build: `.github/workflows/desktop-build.yml`.
- `templates/` directory contains the AI context template files (ai/ skeleton, CLAUDE.md, .cursorrules, etc.) mounted at `/app/templates:ro` in Docker.

# Changelog

All notable changes to Kronn will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.3.1] — 2026-04-01

### Added
- **Usage dashboard** — new "Usage" section in Settings with summary cards (total tokens, estimated cost, discussions, workflows), provider breakdown bar, per-project horizontal bars, and daily history chart (30 days, stacked by provider). Toggle between token count and USD cost view. Filter by discussions, workflows, or all
- **Per-message cost tracking** — `cost_usd` column on `messages` table (migration 024). Real cost captured from Claude Code's `result` stream event; fallback to static pricing estimation for other providers
- **Static pricing engine** — `core/pricing.rs` with per-provider token pricing (Anthropic, OpenAI, Google, Mistral, Amazon). Used when real cost is unavailable
- **Daily usage history API** — `GET /api/stats/tokens` now returns `daily_history` with per-day token/cost breakdown by provider (last 30 days)
- **Discussion deep-link from Usage** — clicking a discussion name in the Usage top-5 list navigates directly to the discussion page and opens it

### Changed
- **Usage centralized in Settings** — the per-agent "Estimated token usage" section in Config > Agents has been removed. All usage data is now in the dedicated Usage section with richer visualizations
- **`StreamJsonEvent::Usage`** — `cost_usd: Option<f64>` integrated directly into the `Usage` variant; the separate `Cost` variant has been removed

---

## [0.3.0] — 2026-03-31

### Added
- **Workflow suggestions from MCP introspection** — `GET /api/projects/:id/workflow-suggestions` matches installed MCPs against a catalogue of 10 workflow templates (orphan PR detection, sprint digest, changelog, stale PRs, bug reports, PR quality, 5xx correlation, sprint brief, perf monitoring, doc sync). Each suggestion includes multi-step prompts, pre-filled trigger, and audience tag (dev/pm/ops)
- **Suggestion panel in workflow wizard** — sparkle button shows contextual workflow suggestions when a project with MCPs is selected. "Activate" (simple mode) or "Import as draft" (advanced mode). Multi-step or advanced suggestions auto-switch to advanced mode
- **Workflow wizard: simple mode** — new 3-step wizard (Infos, Task, Summary) alongside the existing 5-step advanced mode. Toggle at the top of the wizard. Simple mode: one agent, one prompt, manual or scheduled trigger
- **Scheduled trigger in simple mode** — "Manual" or "Schedule" toggle with visual frequency picker (every X minutes/hours/days). Converts to cron behind the scenes
- **System tray (desktop)** — closing the window hides to tray instead of quitting. Backend + workflow scheduler keep running. Double-click tray icon to reopen. "Quit" in tray menu for real exit
- **Wake lock (desktop)** — when cron workflows are active, prevents OS sleep. Windows: `SetThreadExecutionState`. macOS: `caffeinate -w`. Auto-releases when no cron workflows remain
- **MCP audit introspection (step 8)** — audit now calls read-only MCP tools to discover capabilities (tool inventory, project context: Jira projects, GitHub repos, Slack channels, etc.) and documents them in `ai/operations/mcp-servers/`. Generates workflow automation hints table
- **MCP drift auto-detection** — adding/removing/relinking a plugin on an audited project invalidates the `.mcp.json` checksum, flagging drift for step 8 re-run
- **Ad-hoc codesigning for macOS** — CI applies `codesign --force --deep -s -` when no Apple Developer certificate is configured. Release notes include `xattr -cr` instructions

### Changed
- **MCP renamed to "Plugins"** — all user-facing labels (FR/EN/ES), nav tab, page title ("Plugins (MCP / API)"), icons (Server -> Puzzle). Internal code keys unchanged
- **Plugin registry: card grid with category pills** — replaces the flat scrollable list. Cards with icon, name, description (2-line clamp), "Setup required" label. Category filter pills matching Config tab style (border-radius: 20px)
- **Installed plugins: inline expand** — click a plugin card to expand the detail panel in-place (grid-column: 1/-1), no CLS. Shows tokens, scope toggles, project links. Replaces the old accordion-by-server and the above/below detail panel
- **Plugin detail from project page** — clicking a plugin in ProjectCard navigates to Plugins tab and opens the detail panel for that specific config
- **Workflow wizard: advanced options hidden** — concurrency, workspace hooks moved behind "Advanced" toggle in the Config step. Per-step settings (model, retry, stall timeout) were already behind a toggle
- **Audit templates enriched** — `TEMPLATE.md` adds Capabilities table (tools, read-only flag, use-cases) and Project context section. `mcp-servers.md` adds Key capabilities column and Workflow automation hints table

### Breaking (internal)
- **Structured inter-step contract** — new `StepOutputFormat` enum (`FreeText` | `Structured`) on `WorkflowStep`. When `Structured`: engine auto-injects `---STEP_OUTPUT---` envelope instructions, extracts JSON envelope (`{data, status, summary}`) from output, exposes `{{previous_step.data}}`, `{{previous_step.summary}}`, `{{previous_step.status}}` in addition to raw `{{previous_step.output}}`. Includes repair prompt fallback when LLM doesn't comply. Existing workflows unaffected (default = `FreeText`)
- **Catalogue multi-step prompts** — all 10 workflow templates now have 2-4 specialized steps. Collection steps use `Structured` format with explicit data schema in the prompt. Synthesis steps use `FreeText`. Steps reference `{{previous_step.data}}` for structured data instead of raw output

### Fixed
- **Fastly MCP broken** — `fastly-mcp-server` v2.0.x switched to bun runtime. Pinned to v1.0.4 (Node.js) in registry + all 21 `.mcp.json` files across 7 repos. Backend test `pinned_packages_are_respected` prevents regression
- **`PINNED_PACKAGES` dead_code warning** — moved constant into `#[cfg(test)]` module
- **ProjectCard: Server icon → Puzzle** — consistent with Plugins rename

---

## [0.2.2] — 2026-03-29

### Added
- **Contact network diagnostics** — when adding a contact that's unreachable, the API now diagnoses the cause (Tailscale not active, LAN mismatch, peer offline) and returns a machine-readable code. Frontend shows a contextual toast instead of a generic error (i18n FR/EN/ES)

### Fixed
- **Windows: console windows flashing** — every background command (git, agent detection, npx probes, etc.) spawned a visible cmd.exe window on the Tauri desktop app. New `core::cmd` module applies `CREATE_NO_WINDOW` flag to all 50+ `Command::new` calls across the codebase
- **WSL agents not detected** — `wsl.exe -e which` doesn't load the user's login profile, so npm-installed agents (`~/.local/bin/claude`, etc.) were invisible. Now uses `bash -lc` for correct PATH resolution. Version detection also runs via `wsl.exe` for WSL binary paths
- **WSL repositories not scanned** — git commands failed on `\\wsl.localhost\...` UNC paths because Windows `git.exe` doesn't handle them. Git now runs inside WSL via `wsl.exe -e bash -lc "git -C ..."` for WSL filesystem paths. Scan timeout increased from 10s to 30s for WSL paths (9P filesystem is slow)
- **Desktop/self-hosted: "Cannot connect to backend"** — auth middleware relied on `X-Real-IP` header (set by nginx) to detect localhost. In Tauri desktop mode (no nginx proxy), all requests were treated as remote → 401 Unauthorized. Now also checks the direct peer IP via `ConnectInfo`. Startup timeout increased from 5s to 15s. Frontend auto-retries 5 times (2s interval) before showing the error screen
- **macOS CI codesign crash** — empty `APPLE_CERTIFICATE` secret was still exported as an env var, making Tauri attempt to import a null certificate. Signing env vars are now only exported when non-empty
- **Stale installers in CI artifacts** — cargo cache persisted old `.exe`/`.msi`/`.dmg` files across builds. Bundle directory is now cleaned before each build

### Changed
- **Setup wizard: all steps are now optional** — agents and repository detection steps can be skipped (button switches to "Passer cette étape"). Enables non-developer use cases: global discussions without projects, project creation without git repos
- **App icon** — new Lucide Zap lightning bolt icon (`#c8ff00` on `#0a0c10`) matching the web UI. Generated via `cargo tauri icon` from SVG source. Replaces the old generic icon across all platforms (ICO, ICNS, PNG, Windows Store logos)
- **`core::cmd` module** — centralized `async_cmd()` / `sync_cmd()` helpers replace raw `Command::new()` everywhere (agents, scanner, worktree, git ops, workflows, tailscale, checksums, audit). Single place to enforce cross-platform command behavior
- **WSL host label** — agents found via WSL now show "WSL" instead of "Windows" in the setup wizard (new `via_wsl` flag on `BinaryLocation`)

---

## [0.2.1] — 2026-03-28

### Fixed
- **WS security: first message must be Presence** — non-Presence first messages are now rejected, preventing invite code verification bypass (found by multi-agent audit)
- **Tauri desktop: blank page** — `extract_dir` doubled subdirectory paths (`assets/assets/index.js`). Fix: always use root target for path resolution
- **macOS CI build** — removed `|| ''` fallback on Apple signing secrets that caused empty certificate import to fail
- **Localhost exempt documented as tech debt** — `TD-20260328-localhost-exempt` with rotation plan

---

## [0.2.0] — 2026-03-28

### Added
- **Multi-user P2P chat** — share discussions between Kronn instances via WebSocket. Replicated model: each peer stores a full copy, messages sync in real-time
- **`POST /api/discussions/:id/share`** — share a discussion with contacts, broadcasts `DiscussionInvite` via WS
- **`WsMessage::ChatMessage`** — real-time message relay between peers with idempotent insertion (no duplicates)
- **`WsMessage::DiscussionInvite`** — auto-creates local discussion when a peer shares with you
- **Auto-add peers** — unknown but valid invite codes are auto-accepted as pending contacts (no mutual-add required)
- **Host IP detection for Docker** — `KRONN_HOST_IPS` env var, detected at `make start`, passed to container for accurate invite codes
- **Native skill files** — SKILL.md written to `.claude/skills/`, `.agents/skills/` (Codex), `.gemini/skills/` for progressive agent discovery (~95% token savings vs prompt injection)
- **Native agent profiles** — profiles synced as `.claude/agents/`, `.gemini/agents/`, `.codex/agents/` files
- **CSS design system** — `tokens.css` (83 CSS variables), `utilities.css`, `components.css` + per-page CSS files
- **Pagination API** — `?page=1&per_page=50` on discussions list and workflow runs (backward compatible)
- **Auth by default** — auto-generated Bearer token at first launch. Localhost exempt (no lock-out risk). Peers require token. WS auth via invite code
- **Share button** — in chat header, pick a contact to share the discussion with
- **Shared badge** — green Users icon on shared discussions in sidebar
- **Network feedback** — orange "pending" badge + tooltip on unreachable contacts, "offline" label for disconnected accepted contacts

### Changed
- **DiscussionsPage split** — 3254 → 1218 lines + 6 extracted components (ChatHeader, ChatInput, DiscussionSidebar, NewDiscussionForm, MessageBubble, SwipeableDiscItem)
- **SettingsPage split** — 1944 → 990 lines + 3 sections (AgentsSection, IdentitySection, ProfilesSection)
- **WorkflowsPage split** — 1780 → 373 lines + 3 components (WorkflowWizard, WorkflowDetail, RunDetail)
- **Dashboard split** — 1478 → 674 lines + 2 components (ProjectList, ProjectCard)
- **Backend split** — `projects.rs` 3823 → 1396 + `audit.rs` + `ai_docs.rs` + `discover.rs`. `discussions.rs` 3696 → 2322 + `disc_git.rs`
- **Inline styles extraction** — 1157 → 182 inline styles (dynamic only). All static styles moved to CSS
- **Prompt optimization** — native SKILL.md files use progressive disclosure instead of injecting full content. ~25 token reference prompt vs ~800 tokens full injection
- **WS endpoint** — skips auth middleware (invite code verification in ws.rs instead)
- **Tauri desktop app** — frontend files embedded in binary via `include_dir!` (fixes 404 on Windows/macOS installs)
- **Windows Tauri + WSL** — agents detected and executed via `wsl.exe -e` when running on Windows native. Windows paths auto-converted to WSL paths

### Fixed
- **TTS no sound** — added `media-src blob:` to nginx CSP (audio blobs were silently blocked)
- **Tailscale badge** — now conditional on `advertised_host === tailscale_ip` (badge stayed when switching to LAN IP)
- **French accents** — ~120 i18n strings corrected (détecté, sélectionné, créer, réseau, etc.)
- **Spanish accents** — ~90 i18n strings corrected (configuración, validación, código, etc.)
- **Discussion CTA from Projects** — clicking a discussion in ProjectCard now correctly opens it (was missing `onOpenDiscussion(disc.id)`)
- **Discussion visibility on navigate** — `ensureDiscussionVisible` now waits for `allDiscussions` to load before expanding sidebar groups
- **Test stability** — added `act()` flush in `wrap()` helper across 4 test files to reduce flaky failures

---

## [0.1.2] — 2026-03-25

### Added
- **Worktree unlock/lock** — manual button next to the branch badge to release/re-create the worktree. Lets you `git checkout` the branch in your main repo for testing without archiving the discussion
- **Auto re-lock** — when resuming a discussion whose worktree was unlocked, the worktree is automatically re-created (blocks if the branch is still checked out in the main repo)
- **API endpoints** — `POST /discussions/:id/worktree-unlock` and `POST /discussions/:id/worktree-lock`
- **Git signoff by default** — all commits now include `-s` (Signed-off-by), good practice at zero cost

### Changed
- **Worktrees in project directory** — worktrees are now created in `.kronn-worktrees/` inside the repo instead of `/data/workspaces/` in the Docker container. Visible from the host IDE (PHPStorm, VS Code, etc.)
- **Relative gitdir paths** — worktree cross-references use relative paths so they work both inside Docker and on the host
- **Startup migration** — existing worktrees at `/data/workspaces/` are automatically migrated to the new location on startup

### Fixed
- **GPG sign crash** — `--no-gpg-sign` is now passed when the user does not enable `-S`, preventing failures when `commit.gpgsign=true` is set in the git config but the signing key is missing
- **Worktree gitdir broken on host** — `.git` files in worktrees contained Docker-internal absolute paths (`/host-home/...`), now rewritten to relative paths
- **Branch checkout conflict** — clear error message when the branch is already checked out in the main repo instead of a cryptic git error

---

## [0.1.1] — 2026-03-25

### Added
- **MCP: draw.io** — official jgraph server added to registry (49 built-in servers)
- **MCP popover search** — filter + max-height scroll when > 6 MCPs (Discussions page)
- **MCP context file** — `ai/operations/mcp-servers/drawio.md`
- **Installation guide** — `docs/install.md` (Linux, macOS, Windows/WSL2)
- **ErrorBoundary per zone** — each Dashboard page (Projects, MCPs, Workflows, Discussions, Settings) has its own error boundary with inline retry
- **WorkflowStep metadata** — new `step_type` (Agent/ApiCall) and `description` fields on workflow steps, visible in wizard and summary. Prepares for future de-agentification of mechanical steps
- **Shell completions** — bash and zsh autocompletion for `kronn` CLI commands, auto-installed on first run
- **`make bump V=x.y.z`** — centralized version bump across all files (VERSION, Cargo.toml, package.json, tauri.conf.json, README)
- **CHANGELOG.md** — this file

### Changed
- **orchestrate() refactor** — extracted `run_agent_streaming()` and `run_agent_collect()` helpers, reducing orchestrate from ~625 to ~427 lines
- **Version centralized** — single `VERSION` file at repo root; shell, Rust (`env!`), and frontend (`package.json` import) read from it dynamically
- **Git push/PR: auto-token injection** — GitHub token resolved from MCP configs (encrypted in DB), injected into `gh` and `git push` automatically. SSH URLs rewritten to HTTPS with embedded token — no `gh auth login` or `export GITHUB_TOKEN` needed
- **PR creation: auto-push** — `Create PR` automatically pushes the branch if no upstream exists
- Installation docs simplified: agent install is handled by Kronn's setup wizard, not manual npm commands
- **Workflow runner** — replaced `run.clone()` with lightweight `RunProgressSnapshot`, avoids cloning full run state on every step
- **Error hints** — removed outdated French-only comment (messages were already in English)
- **Multi-arch Docker** — confirmed all Dockerfiles already support amd64 + arm64 natively (base images + arch-aware installs)
- **Zero `as any`** — eliminated all 12 `as any` casts across frontend (workers + tests), replaced with proper types (`VoiceId`, `AutomaticSpeechRecognitionPipeline`, `AgentType`, `AiAuditStatus`, `ToastFn`, `UILocale`)

### Fixed
- **Discussion badge desync** — unseen badge showed false positives when switching away from a discussion with an active agent stream
- **SSH on macOS** — git push now works on macOS Docker Desktop via `/run/host-services/ssh-auth.sock` forwarding
- **`.kronn-tmp/` polluting git status** — added to `.gitignore` + global git excludes in container; retroactive fix on startup for existing projects
- **`.kronn-worktrees/` not gitignored** — same treatment as `.kronn-tmp/`
- **Workflow run progress** — running workflows now show step-by-step progression with current step highlighted, instead of just "Running"
- Test fixtures — replaced project-specific names with generic placeholders
- Tech-debt list cleaned: removed 7 resolved entries

---

## [0.1.0] — 2026-03-24

### Added
- **Multi-agent discussions** — Claude Code, Codex, Vibe, Gemini CLI, Kiro with `@mentions`, debate mode, SSE streaming
- **MCP management** — 3-tier architecture (Server → Config → Project), 48 built-in servers, encrypted secrets (AES-256-GCM), disk sync for all agents
- **Workflow engine** — cron, multi-step multi-agent pipelines, tracker-driven (GitHub), manual triggers, 5-step creation wizard, live SSE progress
- **AI audit pipeline** — 4-state system (NoTemplate → TemplateInstalled → Audited → Validated), 10-step automated analysis, drift detection + partial re-audit
- **Pre-audit briefing** — optional 5-question conversational briefing injected into audit steps
- **Project bootstrap** — create new projects from scratch with AI-guided planning (Architect + Product Owner + Entrepreneur)
- **Tauri desktop app** — native installers for Windows, macOS, Linux (no Docker required)
- **Voice: TTS & STT** — 100% local, Piper WASM (9 voices FR/EN/ES) + Whisper WASM, voice conversation mode
- **5 supported agents** — Claude Code, Codex, Vibe (CLI + direct Mistral API), Gemini CLI, Kiro
- **Agent configuration (3-axis)** — 11 profiles (WHO), 22 skills (WHAT), directives (HOW)
- **ModelTier system** — abstract tier selection (fast/balanced/powerful) resolved per agent
- **Multi-key API management** — multiple named keys per provider with one-click activation
- **Token tracking** — per-message token counting (Claude Code stream-json, Codex stderr)
- **Worktree isolation** — each discussion/workflow in its own git worktree
- **GitHub/GitLab PR management** — create, review, merge from the dashboard
- **Responsive UI** — mobile-friendly layout
- **i18n** — French, English, Spanish (CLI + web)
- **CI pipeline** — GitHub Actions: clippy, cargo test, tsc, vitest, bats, security scan (label-triggered)
- **Security** — Bearer token auth (opt-in), CSP headers, AES-256-GCM for secrets

### Stack
- Backend: Rust (Axum 0.7, tokio, serde, SQLite WAL)
- Frontend: React 18 + TypeScript (Vite 5)
- Type bridge: ts-rs (Rust → TypeScript)
- Container: Docker Compose (backend + frontend + nginx gateway)

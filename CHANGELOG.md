# Changelog

All notable changes to Kronn will be documented in this file.

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning: [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

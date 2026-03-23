# Architecture decisions (why, not what)

This file captures **intentional choices** — patterns that look unusual but are deliberate. It prevents agents from "fixing" things that aren't broken.

## Decisions

| Decision | Why chosen | What NOT to do | Source |
|----------|-----------|---------------|--------|
| SQLite single-file DB (no PostgreSQL) | Self-hosted simplicity, zero-config, WAL mode sufficient for local use | Do NOT migrate to PostgreSQL | `backend/src/db/`, `docker-compose.yml` |
| Inline styles (no CSS framework) | No build complexity, no class name conflicts, theming not needed yet | Do NOT add Tailwind/CSS modules | `ai/inconsistencies-tech-debt.md` (TD-20260306-inline-styles) |
| Single Mutex on SQLite (now with spawn_blocking) | SQLite single-writer model, connection pool adds complexity for local app | Do NOT add r2d2 or deadpool | `backend/src/db/` |
| Agent CLIs spawned as child processes | Universal compatibility, no API lock-in, works with any CLI agent | Do NOT use agent SDKs/APIs directly | `backend/src/workflows/`, `backend/src/api/` |
| All ai/ files in English | Universal agent compatibility, consistent context regardless of project language | Do NOT write ai/ files in project language | `ai/index.md` |
| Economy tier for summaries | Save tokens, summary doesn't need reasoning capability | Do NOT use default/reasoning tier for background summaries | `ai/index.md` (§ Stack — ModelTier system) |
| Template redirectors are autonomous (not just redirects) | Small-context agents can't follow redirects; they need actionable content inline | Do NOT revert to 2-line redirect format | `templates/CLAUDE.md`, `templates/.cursorrules`, etc. |
| Agent stall timeout configurable (1–60 min, default 5) | Different agents/tasks need different timeouts; hard-coded 5min too short for audits, too long for simple chats | Do NOT hard-code stall timeout; always read from ServerConfig | `backend/src/agents/runner.rs`, `backend/src/api/setup.rs` |
| TTS/STT 100% local (Piper + Whisper WASM) | Privacy-first, no cloud dependency, works offline, no API costs | Do NOT add cloud TTS/STT providers (Google, Azure, etc.) | `frontend/src/lib/tts-*.ts`, `frontend/src/lib/stt-*.ts` |
| Tauri desktop: backend embedded, not sidecar | Simpler deployment, single binary, no process management, no port conflicts | Do NOT use Tauri sidecar pattern for the backend | `desktop/src-tauri/` |

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

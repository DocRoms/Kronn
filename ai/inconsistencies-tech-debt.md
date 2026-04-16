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

| ID | Problem | Area | Severity |
|----|---------|------|----------|
| TD-20260314-no-tls | No TLS/HTTPS — nginx TLS setup pending. Tailscale encrypts P2P traffic as interim. Not blocking for local/VPN use. | Infra | Medium |
| TD-20260314-no-api-docs | No OpenAPI/Swagger API documentation | Docs | Medium |
| TD-20260318-token-tracking-incomplete | Token usage returns 0 for Gemini CLI and Vibe — SDK doesn't expose token counts. Blocked by upstream. | Backend | Medium |
| TD-20260314-home-mount | `$HOME` mounted read-only in container — security + portability risk | Infra | Low |
| TD-20260328-localhost-exempt | Auth middleware skips localhost + Docker bridge IPs. Pragmatic for self-hosted but needs: (1) token rotation mechanism if leaked, (2) removal when TLS generalized. See `lib.rs:auth_middleware`. | Security | Low |
| TD-20260328-discussions-backend | `discussions.rs` partial progress (2026-04-17): 3400L→2880L. Extracted `disc_helpers.rs` (9 pure helpers, 15 tests) + `disc_prompts.rs` (3 prompt builders + OrchestrationContext, 9 tests). SSE/streaming/cancel core still coupled — separate session needed to split the handler layer. | Backend | Medium |
| [TD-20260417-models-monolith](tech-debt/TD-20260417-models-monolith.md) | `backend/src/models/mod.rs` (~2225L, 147 types) — single file for all data models + ts-rs source. Mechanical split planned (15 sub-modules by domain). | Backend | Medium |
| [TD-20260417-audit-monolith](tech-debt/TD-20260417-audit-monolith.md) | `backend/src/api/audit.rs` (~1966L) — 10-step AI-audit pipeline + drift + validation + briefing in one file. Needs `AuditEngine` abstraction before split. | Backend | Medium |
| [TD-20260417-projects-monolith](tech-debt/TD-20260417-projects-monolith.md) | `backend/src/api/projects.rs` (~1819L) — CRUD + bootstrap + clone + git ops + PR creation. Straightforward sub-directory split planned. | Backend | Medium |
| TD-20260329-toast-no-warning | Toast system only supports `success`, `error`, `info` — no `warning` type. Contact diagnostics use `info` as workaround. Low priority cosmetic. | Frontend | Low |
| ~~TD-20260403-mcp-enabledlist-stale~~ | ~~FIXED 0.3.7~~ — `sync_claude_enabled_servers()` now replaces whitelist entirely | Backend | ~~Low~~ |
| ~~TD-20260403-mcp-naming-migration~~ | ~~FIXED 0.3.7~~ — whitelist replacement removes stale server.name keys automatically | Backend | ~~Medium~~ |


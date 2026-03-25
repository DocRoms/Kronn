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
| TD-20260314-no-tls | No TLS/HTTPS — secrets in cleartext on network. Documented in README. nginx TLS setup pending. | Infra | High |
| TD-20260318-no-auth-by-default | API is unauthenticated until user manually sets a Bearer token — no default protection | Backend | High |
| TD-20260314-no-pagination | No pagination on list_discussions / list_runs / list_projects | Backend | Medium |
| TD-20260314-no-api-docs | No OpenAPI/Swagger API documentation | Docs | Medium |
| TD-20260318-token-tracking-incomplete | Token usage returns 0 for Gemini CLI and Vibe (TODO in runner.rs) | Backend | Medium |
| TD-20260318-large-pages | DiscussionsPage (2325L), WorkflowsPage (1977L), SettingsPage (1874L), Dashboard (1489L) — monolithic | Frontend | Medium |
| TD-20260318-drift-detection | Audit drift detection via `ai/checksums.json` — see `ai/tech-debt/TD-20260318-drift-detection.md` | Backend + Frontend | Feature |
| TD-20260306-inline-styles | All styles are inline — no theming or consistency system | Frontend | Low |
| TD-20260314-polling-heavy | Frontend polls discussions every 15s. WebSocket/SSE push still planned. | Frontend + Backend | Low |
| TD-20260314-home-mount | `$HOME` mounted read-only in container — security + portability risk | Infra | Low |
| TD-20260318-console-errors-prod | 51 console.error() left in frontend production code (intentional for v0.1.0 debugging) | Frontend | Low |

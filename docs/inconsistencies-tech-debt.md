# Inconsistencies & tech debt (index)

> Entry point: `docs/AGENTS.md`. Details: `docs/tech-debt/<ID>.md`.

## Purpose
- A shared list (human + AI readable) of **known inconsistencies** and **things that should be improved**.
- This file is **track-only** — it exists to prevent large sweeping changes by AI and to help create tickets.
- **Details are in individual files** under `docs/tech-debt/`. Only load a detail file when working on that specific topic.

## How to add an entry
1. Create `docs/tech-debt/TD-YYYYMMDD-short-slug.md` using the template below.
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

> Only OPEN items live here. Once a TD is fully fixed, drop the row —
> git history (and the linked detail file, if any) keeps the receipt.
> Partial fixes stay until the residual work is closed out.

| ID | Problem | Area | Severity |
|----|---------|------|----------|
| TD-20260314-no-tls | No TLS/HTTPS — nginx TLS setup pending. Tailscale encrypts P2P traffic as interim. Code-side opt-out shipped: `server.auth_strict_localhost = true` requires Bearer token even on loopback. Migration plan in `docs/operations/auth-and-tls.md`. Not blocking for local/VPN use. | Infra | Medium |
| TD-20260314-openapi-coverage | OpenAPI scaffold shipped (utoipa + Swagger UI at `/api/docs`); 1 of ~170 routes annotated. Residual = incremental enrichment by feature owner — every PR that touches an endpoint adds a `#[utoipa::path]` macro. | Docs | Low |
| TD-20260318-token-tracking-incomplete | Token usage returns 0 for Gemini CLI and Vibe — SDK doesn't expose token counts. Blocked by upstream. | Backend | Medium |
| TD-20260509-react19-effect-rules | Current ESLint baseline is 161 warnings / 0 errors (2026-07-24): 58 `set-state-in-effect`, 27 `immutability`, 22 Fast Refresh, 18 `exhaustive-deps`, 13 `refs`, 12 non-null assertions, 4 `purity`, plus a small tail. Keep directory-sized passes and do not hide the inventory. | Frontend | Medium |
| TD-20260626-export-residuals | Whole-DB export v5 now preserves Quick Prompt version snapshots and learning rejection counters. Residual: uploaded `context_files` rows and their on-disk blobs are still absent from the ZIP. Secrets remain stripped by design. | Backend | Low |
| TD-20260627-configurable-docs-dir | Project docs root is hardcoded-detected (`docs/`→`doc/`→`ai/`) across detection, audit/bootstrap writers, AGENTS.md wiring + frontend viewer. Make it configurable: global default `docs`, optional per-project override, single source of truth threaded everywhere (legacy `ai/`/`doc/` kept as read fallback) — also lets Kronn adapt AGENTS.md + generated files to the chosen folder. | Backend / Config / Frontend | Medium |
| TD-20260629-e2e-seed-vs-real-db | Several Playwright E2E specs are calibrated on a freshly-seeded DB (a11y `a11y-baseline.json` element-count baselines, `audit-banner-lifecycle` "no audit in progress" precondition, 10 s introspection budget) → false reds against a real rich/stateful DB (proven on WSL pre-merge: 8/72 fail, identical on `main`). CI is unaffected (seeds fresh). Fix: state-relative a11y baselines, force a known audit state in setup, bump cold-first-call budget to ~20 s. | CI / Frontend | Low |
| TD-20260629-p2p-native-binding | Contacts/P2P can't work natively: native binds `127.0.0.1` (only Docker forces `0.0.0.0`), `network-info` advertised `127.0.0.1`/empty `detected_ips`, and contact status is reachability-only (no peer-side accept). Shipped: `KRONN_HOST` + UdpSocket LAN-IP detection + a Settings "Allow connections from other devices" toggle (`net_expose`, secure-by-default, Tauri restart button; desktop now honors `server.host`). Remaining: peer auth for non-localhost ops (beyond `/health`), WSL guidance (portproxy/Tailscale), optional live re-bind. Tailscale = recommended path. | Backend / Networking | Medium |
| TD-20260701-print-agent-permission-bridge | Headless Claude Code runs have no interactive approval channel when `full_access` is off; gated tools can stall while the agent invents a permission popup that does not exist. Add an explicit unattended permission policy and structured denial reporting. Detail: `docs/tech-debt/TD-20260701-print-agent-permission-bridge.md`. | Backend / Agents | Medium |
| TD-20260713-graph-delegated-oauth | Scheduled Microsoft Graph `ApiCall` steps need authorization-code + refresh-token OAuth. The broker currently supports client credentials/static exchange only, so the disabled Daily Briefing cannot run unattended. Detail: `docs/tech-debt/TD-20260713-graph-delegated-oauth.md`. | Backend / API broker | Medium |
| TD-20260713-typescript-7-native | Evaluate TypeScript 7's native compiler side-by-side with TypeScript 6; preserve API-dependent tooling and validate diagnostics/performance before cutover. Detail: `docs/tech-debt/TD-20260713-typescript-7-native.md`. | Frontend / Tooling | Low |
| TD-20260715-agent-queue-restart-loss | 0.8.12 shipped owed-run markers, boot interruption notices, cancel-on-delete and queued/running UI. Residual: detached agent work is still not persisted or automatically resumed after restart, and graceful queue draining is absent. Detail: `docs/tech-debt/TD-20260715-agent-queue-restart-loss.md`. | Backend / Agents | High |
| TD-20260715-ws-dead-after-restart-silent-send | After a backend restart, a pre-restart UI tab keeps a dead WebSocket and a message posted from it is **silently dropped** (no error, not persisted) — user needed F5 to recover, and the silent loss mimicked the backend bug (incident #1). Fix: auto-reconnect with backoff + visible "connection lost" state + never drop a send silently (queue/retry or loud failure; ack = persisted). Detail: `docs/tech-debt/TD-20260715-ws-dead-after-restart-silent-send.md`. | Frontend / Realtime | Medium |
| TD-20260717-run-power-assertion-sleep | Full and partial audits now hold and release a macOS `caffeinate` assertion. Residual: ordinary workflow runs and detached discussion-agent runs still have no shared power guard; Linux/Windows implementations are also absent. Detail: `docs/tech-debt/TD-20260717-run-power-assertion-sleep.md`. | Backend / Runs (ops) | High |
| TD-20260721-edit-resend-presence-dispatch | “Edit and resend” deletes trailing replies, mutates the last User row, then calls the deliberately unguarded `/run` escape hatch: a new local CLI agent starts even when MCP peers are connected. Applying the normal presence guard alone would produce silence because the edit creates no newer `sort_order` event; deleted rows also let `MAX(sort_order)+1` reuse a cursor peers have already consumed. Fix requires a monotonic per-discussion event high-watermark plus an atomic presence-aware edit/resend dispatch and federation semantics. Detail: `docs/tech-debt/TD-20260721-edit-resend-presence-dispatch.md`. | Backend / Frontend / Discussions | High |
| TD-20260721-hermetic-notify-e2e | The browser workflow E2E still depends on public `httpbin.org` Notify calls and may skip after a network timeout. Replace it with a hermetic runner-to-Notify harness without weakening production SSRF checks. Detail: `docs/tech-debt/TD-20260721-hermetic-notify-e2e.md`. | Workflows / CI | Low |
| TD-20260721-oxlint-migration | Evaluate Oxlint in a diagnostic-parity dual-run before replacing ESLint; preserve Kronn's project-specific rules and existing warning inventory. Detail: `docs/tech-debt/TD-20260721-oxlint-migration.md`. | Frontend / Tooling | Low |
| TD-20260722-project-scoped-automation-fs | Skills/workflows/QPs/QAs are DB-only with no repo source-of-truth. Full MCP getters for skills/profiles/directives shipped 2026-07-24, closing blind edits; the remaining vision is project-scoped automation as a filesystem (`/skills`, `/automation/{workflows,prompts,api}`) with cross-slug interdependencies, versioned with the code and diffable in PRs. Next: design ADR (source of truth, sync, secrets). Detail: `docs/tech-debt/TD-20260722-project-scoped-automation-fs.md`. | Backend / Skills / Workflows | Medium |
| TD-20260724-planning-and-discussion-plans | Discussions have no shared structured plan and Kronn has no global prioritized task workspace, so objectives and progress remain scattered across transcripts. Validated design: one global task model, discussion plan side panel, compact MCP reads/writes and actor logs. Detail: `docs/tech-debt/TD-20260724-planning-and-discussion-plans.md`. | Backend / Frontend / MCP / Discussions | Medium |

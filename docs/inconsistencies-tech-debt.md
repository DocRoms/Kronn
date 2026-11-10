# Inconsistencies & tech debt (index)

> Entry point: `docs/AGENTS.md`. Details: `docs/tech-debt/<ID>.md`.

## Purpose
- A shared list (human + AI readable) of **known inconsistencies** and **things that should be improved**.
- This file is **track-only** — it exists to prevent large sweeping changes by AI and to help create tickets.
- **Details are in individual files** under `docs/tech-debt/`. Only load a detail file when working on that specific topic.

## How to add an entry
1. Create `docs/tech-debt/TD-YYYYMMDD-short-slug.md` using the template below.
2. Add a one-line summary to the list in this file.

When an item is fixed, remove both its row and its detail file in the same
change. `backend/scripts/ci/tech_debt_registry.py` (CI) checks that every row
has a detail file, every detail file has a row, and neither says it is closed.

## Entry template (for detail files)
- **ID**: TD-YYYYMMDD-short-slug
- **Area**: (e.g. Backend | Frontend | CI | Config | Docs | Other)
- **Problem (fact)**: ...
- **Why we can't fix now (constraint)**: ...
- **Impact**: dev friction | test fragility | perf | security | correctness | docs
- **Where (pointers)**: files/paths/targets
- **Suggested direction (non-binding)**: ...
- **Disposition**: `Scheduled — KT-<n>`, or `Accepted` with its own
  `**Reason**:` and `**Reopen when**:` lines (the check enforces both). A new
  feature is not debt: it goes to Planning, with a design note under
  `docs/design/` when it needs one.
- **Next step**: ticket link, or 'none while accepted'

## Current list

> Only OPEN items live here. Once a TD is fully fixed, drop the row —
> git history (and the linked detail file, if any) keeps the receipt.
> Partial fixes stay until the residual work is closed out.

| ID | Problem | Area | Severity |
|----|---------|------|----------|
| TD-20260314-no-tls | **Reduced by KT-1596**: exposure beyond loopback now needs a declared TLS-terminating proxy (Settings refuses the toggle, a stale config boots on loopback, Docker refuses to start), invites advertise https/wss behind it. **Residual**: Kronn serves no TLS itself — rustls with a generated cert and the fingerprint in the invite is not built (`rcgen`/`axum-server` not vendored). Detail: `docs/tech-debt/TD-20260314-no-tls.md`; runbook `docs/operations/auth-and-tls.md`. | Infra | Low |
| TD-20260314-openapi-coverage | **Accepted.** The OpenAPI scaffold (utoipa + Swagger UI at `/api/docs`) documents only the few handlers listed in `ApiDoc`; the rest is enriched when a PR touches an endpoint. Reason: nearly every route is internal (UI/CLI) and external integrators are rare. **Reopen when**: an external integrator needs a documented endpoint, or an endpoint meant for outside callers (webhook receiver) ships. Detail: `docs/tech-debt/TD-20260314-openapi-coverage.md`. | Docs | Low |
| TD-20260318-token-tracking-incomplete | Token usage returns 0 for Gemini CLI and Vibe — SDK doesn't expose token counts. Blocked by upstream. | Backend | Medium |
| TD-20260509-react19-effect-rules | The CI ESLint ratchet is now 63 warnings / 0 errors (`--max-warnings 63`; 72 at the 2026-08-11 pass). A 2026-11-10 run counted 55, mostly `set-state-in-effect` (36) and `immutability` (9). `exhaustive-deps` and Oxlint are clean; keep contextual directory-sized passes and do not hide the residual inventory. Detail: `docs/tech-debt/TD-20260509-react19-effect-rules.md`. | Frontend | Medium |
| TD-20260626-export-residuals | Whole-DB export v8 now carries uploaded context files (rows in data.json, bytes as optional `context-files/<id>` ZIP entries counted against the import budget) next to Quick Prompt versions and learning rejection counters. Residual: Planning (`planning_*`), Live Pages (`live_page*`) and task executions (`task_execution*`, `orchestration_*`) are still absent from the export; follow-up tickets to be filed. Secrets remain stripped by design. | Backend | Low |
| TD-20260701-print-agent-permission-bridge | Headless Claude Code runs have no interactive approval channel when `full_access` is off; gated tools can stall while the agent invents a permission popup that does not exist. Add an explicit unattended permission policy and structured denial reporting. Detail: `docs/tech-debt/TD-20260701-print-agent-permission-bridge.md`. | Backend / Agents | Medium |
| TD-20260713-graph-delegated-oauth | **Accepted.** Scheduled Microsoft Graph `ApiCall` steps need authorization-code + refresh-token OAuth, and the broker supports client credentials/static exchange only, so the Daily Briefing cron stays disabled. Reason: one personal workflow depends on it, and rebinding to the M365 MCP was rejected as not equivalent; the manual route is an Agent step over the MCP after device-code consent. **Reopen when**: a scheduled workflow must read personal-scope Microsoft data unattended, or any other plugin needs a delegated (user-consent) OAuth flow. Detail: `docs/tech-debt/TD-20260713-graph-delegated-oauth.md`. | Backend / API broker | Medium |
| TD-20260717-run-power-assertion-sleep | **macOS covered**: audits, workflow runs and detached discussion-agent runs share one refcounted `caffeinate` assertion (`core::power_guard`). **Residual**: Linux and Windows hold no assertion yet (`systemd-inhibit` / `SetThreadExecutionState`). Detail: `docs/tech-debt/TD-20260717-run-power-assertion-sleep.md`. | Backend / Runs (ops) | High |
| TD-20260807-agent-config-load-unverified | **Accepted.** Kronn writes MCP config into each agent's own config file and never verifies the agent loaded it. The proven instance (Vibe's workspace-trust store dropping a `.vibe` config, so zero MCP servers load) has detection since 2026-08-08 (sync-time warning, agent runtime warning, `kronn doctor`). Reason: Kronn must not repair another tool's trust store, and no general readback exists; the same class of silent rejection stays unprobed for Codex/Gemini/Kiro/Copilot. **Reopen when**: a second agent (Vibe was the first) is shown to ignore a config Kronn wrote, which makes the per-agent `verify_config_loadable` probe worth building, or an agent offers a cheap status command to read back its loaded servers. Detail: `docs/tech-debt/TD-20260807-agent-config-load-unverified.md`. | Backend / Agents / MCP | Medium |
| TD-20260808-http-agents-no-tool-calling | **Core shipped 2026-08-09; workflow parity added in 0.9.7**: HTTP agents (Ollama, LiteLLM) call a bounded native `tools`/`tool_calls` catalogue in discussions and Workflow Agent steps. Workflow execution is project-scoped, excludes Planning mutations, retains secret-free call receipts and explicitly reports when no call was recorded. **Residual**: no per-model capability probe, no dedicated tool-exposure policy beyond the existing project/API scope. Detail: `docs/tech-debt/TD-20260808-http-agents-no-tool-calling.md`. | Backend / Agents | Medium |
| TD-20260901-plaintext-connection-credentials | **Mostly fixed by KT-1007 (0.14.3)**: connection and provider keys live in the encrypted `stored_credentials` table and are no longer written to `config.toml`. Residuals, documented in `docs/operations/key-management.md` ("Where secrets live", "Boot order" steps 1, 3, 4 and its closing paragraph): (1) on keychain-less hosts without a verified recovery passphrase, `config.toml` keeps the encryption key next to the data it decrypts, so protection is file permissions only; (2) a locked or missing key at boot leaves pre-migration plaintext credentials in `config.toml` until restored (no new ones written); (3) fixed by KT-1528: a set-aside `config.toml.corrupt.<timestamp>` is encrypted with the instance key (only its key line is kept when no key store holds a key), like every other copy of `config.toml`. **KT-1594**: (1) is flagged in a banner and in Settings → Recovery and leads to a passphrase or `KRONN_ENCRYPTION_KEK`, (2) lists the credentials still in clear, (3) is deletable from Settings → Recovery once every credential in it is verified in `stored_credentials` (strong confirmation); the key kept in `config.toml` in (1) remains. Detail: `docs/tech-debt/TD-20260901-plaintext-connection-credentials.md`. [src: file: backend/src/core/keystore.rs:305-325] [src: file: backend/src/core/credential_store.rs:487-505] [src: file: backend/src/core/config.rs:1109-1121#set_aside_unreadable_config] | Backend / Security | Medium |

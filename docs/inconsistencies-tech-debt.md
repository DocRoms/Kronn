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
| TD-20260509-react19-effect-rules | 99 React 19/20 strict warnings remain (down from 122 on 2026-05-10). Cheap categories cleared (no-non-null-assertion: 0, no-explicit-any: 4 SSE-only). Heavy ones left: 42 `set-state-in-effect`, 20 `exhaustive-deps`, 14 `immutability`, 10 `refs`, 4 `purity`, 2 `preserve-manual-memoization`. Per-directory passes from here. | Frontend | Medium |
| TD-20260510-codex-mcp-sandbox-block | Codex 0.121 sees `kronn-internal` MCP and attempts the call but its exec-mode sandbox cancels the spawn (`user cancelled MCP tool call`) — wiring is correct, blocker is upstream. | Backend / agents | Medium |
| TD-20260510-codex-upstream-issue-draft | Draft of the upstream codex-cli issue ready to paste; needs to be filed once. | External | Low |
| TD-20260510-a11y-form-labels | Partial — ~6 inputs labelled (Settings password, Sidebar search, nav tabs, 1 skill input). ~16 more in SettingsPage forms + NewDiscussionForm + WorkflowWizard. Per-form sweep planned. | Frontend | Low |
| TD-20260512-exec-step-worktree-discoverability | Workflow Exec step runs on main tree by default. Worktree mode (`workspace_mode`) buried in Advanced wizard → users running autoBot-style flows get contaminated test results when they have local in-flight work on main. Fix: info banner in wizard when Agent + Exec coexist without worktree. | Frontend | Medium |
| TD-20260512-audit-elapsed-time-display | Audit pipeline runs 2-25 min depending on repo size but UI surfaces no elapsed counter and no historical average. Users can't tell if a long audit is normal or hung. Fix: add `elapsed_ms` to `AuditProgress` events + persist `last_audit_duration_ms` per project. | Frontend | Low |
| TD-20260512-linked-repos-companions | Real projects have companion repos (API backend, IaC, design system) but Kronn audits each in isolation → agents invent cross-repo data. Fix: new `Project.linked_repos: Vec<LinkedRepo>` + Settings UI section + audit Step 1 reads them for context. | Backend | Medium |
| TD-20260512-audit-step9-anti-repetition | Audit Step 9 re-discovers same TDs under new slugs every run; no reconciliation of dropped TDs; `Status` field underused (17/18 Draft); validation discussion stays stale after re-audit. Bundle fix in 0.8.2: read existing TDs as priors + reconciliation pass + two-tier Status (skip already-Verified in Phase 3). | Backend / Audit | Medium |
| TD-20260626-export-residuals | Whole-DB export (`/api/config/export`, `DbExport` v4) covers the high-value tables (incl. quick_apis + learnings since 0.8.9) but still omits: QP version *snapshots* (`quick_prompt_versions` — per-version metrics auto-recompute from discussions, so only the diff history is lost), uploaded `context_files` blobs on disk (only DB rows would round-trip, and even those aren't exported), and `learning_rejections` dedup state. Secrets (API key values + MCP env) are stripped by design. Fix when a migration needs full fidelity: add a faithful raw-insert for qp_versions + bundle on-disk context files into the ZIP. | Backend | Low |
| TD-20260627-configurable-docs-dir | Project docs root is hardcoded-detected (`docs/`→`doc/`→`ai/`) across detection, audit/bootstrap writers, AGENTS.md wiring + frontend viewer. Make it configurable: global default `docs`, optional per-project override, single source of truth threaded everywhere (legacy `ai/`/`doc/` kept as read fallback) — also lets Kronn adapt AGENTS.md + generated files to the chosen folder. | Backend / Config / Frontend | Medium |


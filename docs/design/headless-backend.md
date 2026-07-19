# Design note — Headless backend (`kronn --headless`)

**Status:** proposed (2026-07-19) · **Origin:** MCP dogfooding session — "peut-on utiliser le MCP sans démarrer Kronn ?"

## Problem

The `kronn-internal` MCP bridge is a pure HTTP client: every tool call — even a
read-only `disc_meta` — requires the backend on `localhost:3140`. A user who
opens a CLI to ask "what did we decide in that discussion last week?" must
first boot the full application (repo scanner sweep, cron engine, workflow
engine, WS hub, frontend expectations). When the backend is down, the bridge
degrades into cryptic per-call HTTP errors.

## Rejected alternative: SQL-direct bridge

Letting the bridge read the SQLite database directly was considered and
rejected:

1. **Schema drift** — the bridge would duplicate migration knowledge (75
   migrations and counting). Two sources of truth for the schema is the exact
   bug class the 0.8.12/0.8.13 hardening spent days eliminating (index vs
   files, `done_status` vs DB row…).
2. **Crypto boundary** — the secrets envelope lives in the backend; a
   SQL-only bridge could never resolve credentials, tokens, or encrypted
   config.
3. **Read-only ceiling** — every useful verb (`disc_append`, `qp_run`,
   `audit_launch`, `workflow_trigger`) needs agents, SSE and business logic.
   The standalone mode would cover introspection only, for the full cost of a
   parallel data layer.

## Proposed design: lazy minimal profile

A `--headless` flag (or `KRONN_HEADLESS=1`) boots a **minimal profile** of the
existing binary:

| Subsystem | Full | Headless |
|---|---|---|
| SQLite + migrations | ✅ | ✅ |
| HTTP API | ✅ | ✅ |
| Secrets envelope | ✅ | ✅ |
| Agent spawning | ✅ | ✅ (on demand) |
| Repo scanner sweep at boot | ✅ | ❌ (lazy, per request) |
| Cron/trigger engine | ✅ | ❌ |
| Workflow engine autostart | ✅ | ❌ (runs start on explicit trigger only) |
| WS broadcast hub | ✅ | ✅ (cheap, needed by batch/audit events) |

Target: **< 1 s** from spawn to first successful API call.

The bridge gains a **lazy-start** behaviour: when `_http` gets a connection
refused, it may spawn `kronn --headless` (opt-in via
`KRONN_MCP_AUTOSTART=1` — never by surprise) and retry once. Otherwise it
returns one clear, actionable error: *"Kronn backend is not running — start it
with `make run-backend` (or enable KRONN_MCP_AUTOSTART)"* instead of a raw
HTTP error per call.

## Interactions

- **`bridge_info`** already reports bridge staleness; it should also report
  the backend's reachability + profile (full/headless) so agents can explain
  the environment.
- **Audits/workflows launched against a headless backend** run fine (agents
  spawn on demand); only cron-triggered work requires the full profile. The
  audit drop-guard (0.8.13) and boot reconciles behave identically.
- **Single-instance guard**: headless must refuse to start when a full
  backend already owns the port/db lock (`.kronn.lock`), and a full boot
  should adopt-or-replace a headless one gracefully.

## UI launching from the MCP

Follow-up question from the same session: could the bridge launch the
backend AND the frontend? Two different answers:

- **Backend**: yes — that's the lazy-start above (opt-in).
- **Frontend**: never as a bridge-spawned process. Anything the bridge
  spawns dies with the MCP session (a bridge respawn killed a live audit on
  2026-07-19 — the lesson behind the audit drop-guard). Instead, the
  headless profile should **serve the built frontend statically** (the
  desktop app already bundles it); "open the UI" then becomes a cheap
  `kronn_open_ui` bridge tool: ensure the backend is up (lazy-start) and
  open `http://localhost:3140` in the default browser (`open`/`xdg-open`).
  No extra process, no lifecycle coupling. The onboarding guide
  (`kronn_intro`) points users there for anything secret-related: secrets
  are configured in the UI only, never through the chat.

## Non-goals

- No second data layer, no bridge-side SQL, ever.
- No daemonization/service management (the OS or the operator owns that).

## Sizing

Backend: a boot-path branch gating scanner/cron/workflow autostart (~1 day
incl. tests). Bridge: autostart + error message (~2 h). Candidate: 0.8.14.

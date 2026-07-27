# Plugin hybrid invocation + credential provider

> Status: **accepted / P1 + preferred-interface + secure portability delivered**
> (2026-07-27). Author:
> Claude Code with Codex. Driven by Romu's Fastly dogfood ("Stats Problème ES",
> disc `47c2157f`). The selected credential design is documented in §4.

## 1. Problem

Fastly was configured in Kronn but its MCP tools (`fastly_execute`) never
loaded in a live investigation — the agent could see the config but not call
anything. Root cause: the registry declares Fastly as a **bare Go binary**
(`command: "fastly-mcp"`, `backend/src/core/registry.rs:1510`) with **no env**
(`env_keys: vec![]`), and that binary is never installed by the build. So the
server never ran, with no credentials. Re-installing the *Fastly CLI* did not
help — the CLI is not the MCP server.

Two official generations of the Fastly MCP exist (audited 2026-07-27):

- **`fastly/mcp` v0.1.11** (Go binary `fastly-mcp`) — wraps the Fastly CLI and
  reuses its local auth.
- **`@fastly/mcp` 2.x** (JS) — rewritten to call the Fastly REST API directly
  via `FASTLY_API_TOKEN`; the CLI is no longer required. Runs via
  `npx -p @fastly/mcp fastly-mcp` (needs Node ≥22).

## 2. The bigger insight (Romu)

Kronn's value is **determinism**: with a token, Kronn calls the Fastly REST API
**directly** (the `api_call` broker / Quick APIs / `ApiCall` workflow step) —
zero LLM tokens, reproducible, auth injected server-side and redacted
(`ResolvedAuth`), never exposed to the model. This is "désagentification". The
"Stats Problème ES" case is a deterministic call to Fastly `/stats` by
POP/region — not an agent fumbling with MCP tool-calls.

So the deterministic **API path is the primary value**; the agentic MCP is
secondary (exploration of undeclared operations).

## 3. Design: three invocation modes + a preferred-interface policy

A plugin can expose up to three capabilities, shown separately in the UI:

| Mode | When | Mechanism | Determinism |
|------|------|-----------|-------------|
| **API** | known/repeatable ops (stats, list services, domains, gated purge) | Quick API / `ApiCall` step / `api_call` broker | **deterministic**, 0 LLM tokens, testable, auditable |
| **MCP** | exploration / undeclared ops | official `@fastly/mcp` (agentic `search → inspect → execute`) | non-deterministic |
| **CLI** | manual expertise / fallback | user's `fastly` CLI | manual |

Add a generic per-plugin field **`preferred_interface: API | MCP | CLI`**
(defaults to `API` for Fastly once the deterministic endpoints exist). When the
plugin is linked to a discussion, Kronn injects one short agent-facing rule,
e.g.:

> Fastly — prefer the deterministic API (an existing Quick API, else the API
> broker). Use the MCP only to explore an operation the API doesn't cover. CLI
> is a last-resort fallback.

The selector collapses to the modes actually available (a pure-MCP or pure-API
plugin shows only what it has). This makes the card the single source of truth
for **humans and agents** on how to call the plugin.

## 4. Credential resolution — accepted design

The chosen model keeps the Fastly CLI as the single local credential source:

1. Kronn pins the official Go `fastly-mcp` and Fastly CLI binaries in the
   image, with release checksums verified during the build.
2. The container wrapper points the Linux Fastly CLI at the host's read-only
   config directory (`/host-home/Library/Application Support` on macOS,
   `/host-home/.config` on Linux/WSL).
3. The MCP server shells out to that CLI normally.
4. The deterministic API broker uses the trusted registry-only
   `ApiAuthKind::CliToken` provider to execute `fastly auth token` directly,
   with no shell. It keeps stdout in memory, injects it as `Fastly-Key`, and
   never persists or logs the value.

This is one authentication setup, not dual auth: `fastly auth login` powers
both MCP and deterministic API calls. No token is copied into Kronn's encrypted
DB, Docker environment, prompts, or generated MCP configuration.

`CliToken` is intentionally rejected for custom/imported plugin specs. Only a
built-in registry definition can declare a local credential command; otherwise
an imported JSON file could become an arbitrary-command execution primitive.

## 5. Security

- **Scoped token**: recommend a dedicated `global:read` Fastly token (read-only:
  account/config/**stats** — exactly the audience use case), optionally
  service-limited and with an expiry. Never the user's full-access personal
  token. Write ops (purge/VCL) → a separate scoped token.
- **No leak to the model** (`ResolvedAuth` redaction) and **no leak to files**:
  the provider keeps the token in memory and the CLI-backed MCP needs no token
  field in `.mcp.json`.

## 6. `ApiSpec` for Fastly (deterministic broker)

`ApiSpec { base_url: "https://api.fastly.com", auth: header "Fastly-Key",
endpoints: [stats by service/region, list services, domains, gated purge, …],
docs_url }`. Start with the useful/safe read endpoints; mark write ops
(purge/VCL) explicitly and require confirmation. No need to cover 100% of the
API to get immediate deterministic value.

## 7. Plugins page — export/import + card→drawer homogenization

- **Card → vertical drawer**: delivered as a non-blocking master/detail panel.
  The plugin list reflows, remains clickable, and switches the open panel in
  one click; the panel becomes full-screen on mobile. Sections:
  Overview, Connection/auth, Project scope, Availability in agent CLIs
  (API ✓ · MCP ✓ · CLI ✓), Preferred interface, Agent rule, Diagnostics.
  Sticky footer: Test (a **real probe**, not a fake green — authenticated safe
  GET for API, bounded `initialize` handshake for MCP; a missing trusted probe
  is non-ready), Save, secondary menu.
  Keep modals only for atomic confirmations (delete, raw import/export).
- **Export mode**: pick plugins to export.
  - **Default: config only, no secret values** (teammate supplies their own).
  - **Opt-in "include secret values" = RED danger zone**: explicit checkbox +
    typed confirmation + an explicit list of what leaves.
  - When secrets are included, the bundle is **encrypted with a passphrase**
    (reuse Kronn's argon2/aes-gcm) — not a cleartext token file on Slack.
  - A plugin in credential mode `cli` (Option C, token never stored) **has no
    secret to export** — pushing CLI mode mechanically shrinks the exportable-
    secret surface.
  - The export is **audited** (which plugins, with/without secrets).
- **Import mode**: delivered with an explicit trust boundary. Registry-backed
  plugins are resolved from Kronn's current trusted registry rather than from
  the bundled snapshot; unknown executable/MCP definitions are refused. A
  bundle cannot silently replace an existing configuration, automatically
  become global, attach itself to a project or enable host sync. Exact replays
  are idempotent and changed content using an already-imported bundle id is an
  explicit conflict.

## 8. Phasing + lanes

- **P1 — deterministic Fastly that works** (highest value, minimal): official
  MCP wired correctly + Fastly `ApiSpec` + CLI credential resolution + pinned
  binaries in Docker + a real connection probe. Fixes "Stats
  Problème ES" and delivers determinism immediately.
- **P2 — generic + UX**: the card→drawer homogenization, generic
  `preferred_interface` policy and secure plugin export/import are delivered.
  The remaining work is a generic `CredentialProvider` (reusable gh/glab).

Delivery lanes: **Codex** handled the registry hybrid, `ApiSpec`, trusted
`CliToken` provider, Docker binaries, probe and backend tests. **Claude**
handled the non-blocking drawer and frontend tests. Codex then delivered the
persisted preferred-interface selector, availability validation and the same
compact invocation rule across every agent runtime, followed by the portable
bundle contract, encrypted-value boundary, audit trail and end-to-end UI.

## 9. Remaining decisions

1. Confirm `global:read` scoped token as the recommended default (§5).
2. Continue dogfooding the delivered drawer and probe wording.

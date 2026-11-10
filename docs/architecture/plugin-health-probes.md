# Plugin health probes (KT-829)

A plugin exposes up to three **accesses**: `api`, `mcp`, `cli` (matches
`PluginInterface` / `McpProbeCheck.id`). Health is measured **per access**,
not per plugin — a config can be `ok` on `api` and `cli_not_authenticated`
on `cli` at the same time.

## Diagnostic codes

`ProbeDiagnosticCode` (`backend/src/models/mcp.rs:496`) is the stable,
serializable classification every probe check carries in
`McpProbeCheck.code` (`backend/src/models/mcp.rs:486`). `detail` stays free
English text for logs; `code` is what a frontend should switch on to
translate.

- `unauthorized` / `forbidden` / `not_found` — HTTP 401/403/404, extracted from
  either message shape a probe failure uses (`extract_http_status`,
  `backend/src/api/mcps.rs:394`).
- `invalid_header` — a header built from the config's values could not be
  sent as-is (malformed value, non-ASCII name).
- `unexpected_output` — a local CLI credential resolution produced output
  Kronn could not use (non-UTF-8, empty token, banner noise).
- `network` — connection/DNS/timeout, never reached the server.
- `cli_missing` / `cli_version_too_old` / `cli_not_authenticated` — the three
  CLI-access-probe-specific codes (see below). `cli_missing` is also reused
  for an MCP stdio transport whose binary won't spawn — same failure mode,
  same remediation ("install it").
- `other` — anything else (5xx, unclassified, no probe declared).

The classifier is `classify_probe_failure` (`backend/src/api/mcps.rs:354`),
a pure function tested case-by-case in the same file's test module.

## CLI access probe

`registry::cli_access_probe(server_id)` (`backend/src/core/registry.rs:82`)
declares, per plugin, the three commands a CLI-backed plugin needs checked:
version args, an optional minimum version, and an auth-check command that
exits 0 only when actively logged in (`fastly whoami`, `glab auth status`).

`core::cli_access_probe::probe()` (`backend/src/core/cli_access_probe.rs:33`)
runs those through the Quick Exec engine (`core::quick_exec` — no shell, an
allowlisted binary, a literal argv) and returns one of the four stable codes.
`fastly` and `glab` had to be added to `quick_exec::ALLOWED_BINARIES` for
this — they're presence/version/auth checks only, never a mutating
subcommand.

## Persistence + bulk test

Every probe (`POST /mcps/configs/{id}/probe`, `backend/src/api/mcps.rs:122`)
writes its checks to `mcp_probe_results` (migration
`backend/src/db/sql/197_mcp_probe_results.sql`, one row per
`(config_id, access)`), surfaced back as `McpConfigDisplay.last_probes`
(`backend/src/models/mcp.rs:443`, `McpLastProbe` at `:530`) so the plugin
list shows health without re-probing on load.

`POST /mcps/test-all` (`backend/src/api/mcps.rs:1736`) probes every visible
config with a bounded concurrency (`TEST_ALL_CONCURRENCY`, a `Semaphore`,
same idiom as `workflows::batch_apicall_step`), persisting the same way.

## Refused tokens (KT-1524)

A probe is not the only thing that learns a token is dead: every real call
does. `workflows::api_auth_failure::track_outcome` runs after each ApiCall
(workflow step, batch item, collector source, Quick API, agent broker call,
wizard test) and after each readiness probe:

- success → deletes the connection's row in `mcp_auth_failures` (migration
  `backend/src/db/sql/290_mcp_auth_failures.sql`);
- HTTP 401, a 403 whose body blames the credential (a bare 403 or a rate limit
  is *not* an auth failure), or a rejected OAuth2 / token-exchange mint → upserts
  the row (`kind`, `http_status`, scrubbed provider message, `failed_at`);
- any other failure (5xx, network, timeout) leaves the row as it was.

The row surfaces as `McpConfigDisplay.auth_failure`: the card shows the
`TokenRefusedNotice` with an update action, the list row a badge, the API
access health reads `token_refused`, and `AuthFailureBanner` (dashboard layout)
lists every refused connection. A failed ApiCall output starts with one plain
sentence (`Connection "<label>" (<plugin>) refused the token (HTTP 401): … —
update the token in Settings → APIs.`) and ends with `[SIGNAL: auth_refused]`;
a `CollectApiData` step repeats it with the `N of M sources` count. The run page
(`RunDetail`) keys its run-level notice on that signal.

Not covered: a token's *known expiry date* — connections store none, so an
expiring token is only caught by the call (or probe) that gets refused.

## Rescan report

`POST /mcps/refresh` (`backend/src/api/mcps.rs:1474`) now returns
`McpRescanReport` (`backend/src/models/mcp.rs:754`): counts of configs
created / merged / deleted, and (for a real run) how many projects' files
were actually rewritten. `?dry_run=true` runs the entire scan inside one
SQLite transaction and rolls it back instead of committing — nothing is
written to the database or the filesystem (`mcp_scanner::sync_all_projects`
is skipped entirely, so `projects_rewritten` is `None` for a dry run).

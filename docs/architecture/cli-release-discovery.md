# CLI release discovery

Kronn separates the installed CLI version from the last known stable release.
Release discovery uses an in-memory, six-hour cache, including failed attempts.
Normal agent detection schedules a stale refresh without waiting for network
I/O. Settings reads await that shared refresh asynchronously; an explicit
`POST /api/agents/version-check` bypasses the TTL, but joins an already-running
refresh. No release number is a compiled-in authority.
[src: file: backend/src/core/versions.rs:13-16#CACHE_TTL]
[src: file: backend/src/core/versions.rs:210-234#refresh_if_stale_and_wait]
[src: file: backend/src/api/agents.rs:36-50#version_check]

The adapters query npm's `/{package}/latest` version document (not its unbounded
all-versions document), PyPI JSON, or GitHub's latest-release endpoint. Each
request has a five-second timeout, the batch an eight-second budget, and each
response a four-MiB limit enforced both on Content-Length and streamed chunks.
Redirects are refused. Invalid numeric releases, prereleases and GitHub drafts
are not accepted. A source failure keeps the last valid release and adds an
explicit error; unknown is never presented as proof of being up to date.
[src: file: backend/src/core/versions.rs:36-43#Source]
[src: file: backend/src/core/versions.rs:293-426#fetch_stable_at]
[src: url: https://github.com/npm/registry/blob/main/docs/REGISTRY-API.md]

The shared refresh owns its lifetime independently of any HTTP caller.
Cancelling the first caller does not strand other waiters. A dropped refresh
lease clears the in-flight flag and notifies waiters; waits have their own
upper bound. `checked_at` is the **last attempt**, not the last successful
verification: after an error it can accompany an older retained release.
The cache is not durable across backend restarts.
[src: file: backend/src/core/versions.rs:106-163#RefreshLease]
[src: file: backend/src/core/versions.rs:236-282#apply_results]

RTK and ccUsage use this same release snapshot. Their installed versions are
separate read-only `--version` probes, each limited to three seconds and eight
KiB of stdout; nonzero exit, invalid output or timeout yields unknown. The
owned child is killed on cancellation and killed/reaped after a failed probe.
No package runner, installer or updater is invoked by discovery.
[src: file: backend/src/core/versions.rs:428-470#probe_installed_version]
[src: file: backend/src/api/rtk.rs:431-478#ccusage_status]

ccUsage probes the **same resolved executable** used by Kronn's usage reports:
an explicit executable, PATH/conventional installation, or an existing npm,
pnpm or Bun cache. Platform-native `@ccusage/ccusage-*` packages and JS shims
are installation variants of that unified executable, not separate Codex
release sources. This does not assert that RTK's own economics subprocess
resolves exactly the same executable.
[src: file: backend/src/core/usage.rs:322-433#resolve_ccusage_program]
[src: file: backend/src/core/usage.rs:555-571#fetch_usage]

The Settings card shows RTK and ccUsage independently, including ccUsage when
RTK is absent. Errors and attempt dates remain visible beside retained versions;
manual rechecks are guarded against synchronous double clicks, and request
failures do not erase the displayed snapshot. An upgrade command is only shown
for the user to run explicitly.
[src: file: frontend/src/components/settings/CompressionSection.tsx:89-121#recheckVersions]
[src: file: frontend/src/components/settings/CompressionSection.tsx:268-287#config.releaseCheckFailed]

Regression coverage includes overlapping/cancelled callers, cache TTL and
timeouts, runtime shutdown, local HTTP error/oversize/chunked fixtures, fake
CLI arguments and bounded output, the real HTTP envelope, and Settings rechecks.
The shared frontend API mock includes the generated RTK response shape because
the version read is no longer conditional on an installed RTK binary.
[src: file: backend/src/core/versions.rs:512-722#concurrent_callers_share_refresh_even_after_first_waiter_is_cancelled]
[src: file: backend/tests/api_tests.rs:202-233#cli_release_recheck_returns_agent_metadata_and_distinct_rtk_ccusage_versions]
[src: file: frontend/src/test/apiMock.ts:603-608#RtkVersionInfo]

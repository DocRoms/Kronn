# Backend test host-sync isolation

## Host-sync entry points

The MCP create, update, delete, and agent-change handlers invoke
`sync_affected_projects` or `sync_all_projects`; the former enumerates the
Codex, Copilot, Claude, and Gemini host adapters. Any router fixture that
exercises those MCP handlers therefore needs both owned configuration and host
roots. [src: file: backend/src/api/mcps.rs:650-673]
[src: file: backend/src/api/mcps.rs:1180-1192]
[src: file: backend/src/api/mcps.rs:1244-1300]
[src: file: backend/src/api/agents.rs:1-18]
[src: file: backend/src/core/mcp_scanner.rs:2228-2259]

`backend/tests/api_tests.rs` is an independent process. Its ordinary
`test_state` calls a single `OnceLock<TempDir>` initializer which creates and
installs both `KRONN_DATA_DIR` and `KRONN_HOST_HOME`; subsequent state builders
do not mutate either process-global variable. [src: file: backend/tests/api_tests.rs:1645-1662]
[src: file: backend/tests/api_tests.rs:2043-2056]

The library test module has a different constraint: its non-serial state
constructors do not mutate the environment, while the pre-existing config-save
fixture remains serial because tests in that binary remove its data override.
[src: file: backend/src/api_tests.rs:24-50]

## Backend test-binary audit

The independent router fixtures for model catalog, continual learning,
orchestration handoff, room routing, discussion target selection, and context
file import each install data and host roots once, inside their process-lived
`OnceLock<TempDir>` initializer. [src: file: backend/tests/model_catalog_api.rs:24-36]
[src: file: backend/tests/learnings_api.rs:24-36]
[src: file: backend/tests/orchestration_handoff_e2e.rs:27-39]
[src: file: backend/tests/room_reaches_the_agent_e2e.rs:26-38]
[src: file: backend/tests/discussion_target_model.rs:12-28]
[src: file: backend/tests/context_file_safety.rs:13-24]

The remaining binaries were checked for `AppState` construction, router
creation, configuration-root overrides, and MCP sync calls. `http_probe_catalog`
and `ollama_model_catalog` construct a router for external-model probe/catalog
routes; their fixtures contain no MCP configuration route. `http_model_resolution`
uses an isolated data root but no `AppState`; `model_catalog_migration` uses its
own database; `real_agent_e2e` is ignored and has no backend state fixture.
[src: file: backend/tests/http_probe_catalog.rs:1-78]
[src: file: backend/tests/ollama_model_catalog.rs:55-75]
[src: file: backend/tests/http_model_resolution.rs:1-58]
[src: file: backend/tests/real_agent_e2e.rs:1-80]

The explicit config-write guard removes only `KRONN_DATA_DIR` to assert a
rejected configuration write; it does not build a router or invoke an MCP
adapter. [src: file: backend/tests/config_write_guard.rs:1-11]

## Unit-test audit

The generic MCP-scanner file fixture does not set or clear host resolution.
Direct host-sync planner tests use `#[serial]`, own a temporary host directory,
and restore their prior host override. The receipt regression retains its
Written, Unchanged, and ReadOnly assertions with RAII restoration of every
override it changes. [src: file: backend/src/core/mcp_scanner_test.rs:43-55]
[src: file: backend/src/core/mcp_scanner_test.rs:2948-3022]
[src: file: backend/src/core/mcp_scanner_test.rs:3025-3103]
[src: file: backend/src/core/mcp_scanner_test.rs:3105-3193]
[src: file: backend/src/core/mcp_scanner.rs:5247-5306]

Host-discovery and agent-auth unit tests that change `KRONN_HOST_HOME` are
serial and restore the saved value; they exercise discovery/auth reads rather
than the MCP host-sync adapters. [src: file: backend/src/core/host_mcp_discovery.rs:797-810]
[src: file: backend/src/agents/mod.rs:1208-1236]

## Synthetic router regression

The real MCP creation handler runs in a fresh child process with only
disposable outside sentinels for every adapter. The RED child bypasses the
normal state fixture and must modify a sentinel. The GREEN child relies only on
ordinary `test_state` initialization, verifies every sentinel remains unchanged,
and sends two ordinary concurrent create requests after initialization. The
blocked child places a file inside the owned Codex fixture and verifies that a
write failure leaves the outside sentinels unchanged. The parent uses the
cross-platform command helper, has a deadline/kill path, and asserts that its
own two overrides are unchanged after every child. [src: file: backend/tests/api_tests.rs:9933-10140]

The parent initializes the ordinary `OnceLock` before taking those environment
snapshots. Otherwise a parallel non-serial router test can perform the first
initialization during a child run and make the parent falsely report an
environment leak. Keep that initialization inside the parent branch: doing it
in the child before its RED control would hide the confinement regression.
[src: file: backend/tests/api_tests.rs:9974]

The focused regression is not a replacement for the principal's combined-suite
qualification; it is the source-level confinement audit and regression proof
for the host-sync paths above.

# Backend test host-sync isolation

## Audit

The MCP create handler calls `sync_affected_projects`; MCP updates call either
`sync_all_projects` or `sync_affected_projects`. Therefore router tests that
create or update MCP configurations can reach the host-sync adapters.
[src: file: backend/src/api/mcps.rs:650-673]
[src: file: backend/src/api/mcps.rs:1180-1192]

The integration API harness now keeps a unique `TempDir` alive for the process
and resets both `KRONN_DATA_DIR` and `KRONN_HOST_HOME` to its `data` and
`host-home` children whenever it builds state. The library API harness has the
same two-root initialization on both state constructors. This makes a later
test removal of the host override recover to an owned path when a harness is
used again.
[src: file: backend/tests/api_tests.rs:1645-1667]
[src: file: backend/src/api_tests.rs:24-54]

The independent model-catalog, learnings, orchestration-handoff,
room-reaches-agent, and discussion-target router fixtures were audited as
separate binaries and now set both roots to process-owned `TempDir` children.
Their test-state construction is adjacent to the override in each fixture.
[src: file: backend/tests/model_catalog_api.rs:24-46]
[src: file: backend/tests/learnings_api.rs:24-47]
[src: file: backend/tests/orchestration_handoff_e2e.rs:27-51]
[src: file: backend/tests/room_reaches_the_agent_e2e.rs:26-48]
[src: file: backend/tests/discussion_target_model.rs:12-60]

The MCP scanner unit helper previously removed `KRONN_HOST_HOME`; it now resets
that variable to a process-owned temporary fixture instead. Its local-path
test continues to exercise an existing local path after this initialization.
[src: file: backend/src/core/mcp_scanner_test.rs:43-53]
[src: file: backend/src/core/mcp_scanner_test.rs:1366-1378]

The explicitly unisolated config-write guard remains an intentional negative
test: it removes only `KRONN_DATA_DIR` and asserts that `config::save` refuses
the write. It does not invoke an MCP handler or host-sync adapter.
[src: file: backend/tests/config_write_guard.rs:1-11]

## Router regression

The MCP regression launches a fresh copy of the API test binary with a
synthetic outside `KRONN_HOST_HOME` sentinel. The child runs the real
`POST /api/mcps/configs` handler with `GlobalOnly`; it must replace the
inherited value through the harness, write Codex and Copilot configuration only
under the owned fixture, and leave the outside sentinel unchanged. Its parent
executes both synthetic controls: an unisolated child changes its disposable
sentinel (RED), then the normal harness child leaves a different disposable
sentinel unchanged (GREEN).
[src: file: backend/tests/api_tests.rs:9938-10007]

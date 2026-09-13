# Backend test host-sync isolation

## Audit

The MCP create handler calls `sync_affected_projects`; MCP updates call either
`sync_all_projects` or `sync_affected_projects`. Therefore router tests that
create or update MCP configurations can reach the host-sync adapters.
[src: file: backend/src/api/mcps.rs:650-673]
[src: file: backend/src/api/mcps.rs:1180-1192]

The ordinary library API state constructors do not mutate process environment:
the config-saving tests call their pre-existing `#[serial]` data-dir helper.
The MCP host-sync router regression instead performs host setup only in a
fresh, serial child process, preserving the parent process environment.
[src: file: backend/src/api_tests.rs:24-50]
[src: file: backend/tests/api_tests.rs:1650-1670]

The independent model-catalog, learnings, orchestration-handoff,
room-reaches-agent, and discussion-target router fixtures were audited as
separate binaries and now set both roots to process-owned `TempDir` children.
Their test-state construction is adjacent to the override in each fixture.
[src: file: backend/tests/model_catalog_api.rs:24-46]
[src: file: backend/tests/learnings_api.rs:24-47]
[src: file: backend/tests/orchestration_handoff_e2e.rs:27-51]
[src: file: backend/tests/room_reaches_the_agent_e2e.rs:26-48]
[src: file: backend/tests/discussion_target_model.rs:12-60]

The generic MCP scanner file helper does not set or clear `KRONN_HOST_HOME`.
Every direct host-sync override in that test module is inside a `#[serial]`
test and restores its previous value. The receipt regression owns its host
fixture and restores all four environment overrides through `Drop`.
[src: file: backend/src/core/mcp_scanner_test.rs:43-52]
[src: file: backend/src/core/mcp_scanner_test.rs:2953-3190]
[src: file: backend/src/core/mcp_scanner.rs:4147-4175]
[src: file: backend/src/core/mcp_scanner.rs:5210-5305]

The explicitly unisolated config-write guard remains an intentional negative
test: it removes only `KRONN_DATA_DIR` and asserts that `config::save` refuses
the write. It does not invoke an MCP handler or host-sync adapter.
[src: file: backend/tests/config_write_guard.rs:1-11]

## Router regression

The MCP regression launches a fresh copy of the API test binary with synthetic
sentinels for all four host adapters: Codex, Copilot, Claude, and Gemini. The
child runs the real `POST /api/mcps/configs` handler with `GlobalOnly`; the
unisolated control changes a disposable sentinel (RED), while the isolated
control leaves every outside sentinel byte-identical (GREEN). A third child
uses a blocking file inside its owned Codex fixture and verifies that an adapter
write failure does not fall back to the outside sentinel. The parent uses the
cross-platform command helper and kills a child that exceeds its deadline.
[src: file: backend/src/core/mcp_scanner.rs:2242-2254]
[src: file: backend/tests/api_tests.rs:9940-10095]

## Remaining qualification boundary

This audit covers the API harness, the five independent router fixtures named
above, and the MCP scanner test helpers. It does not claim that every backend
test binary is host-sync-free; the final combined backend-suite qualification
remains a principal-owned gate. The intentionally unisolated config-write guard
does not invoke an MCP handler or host adapter.
[src: file: backend/tests/config_write_guard.rs:1-11]

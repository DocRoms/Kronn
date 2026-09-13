# Backend test host-sync isolation

Integration-router setup must set both `KRONN_DATA_DIR` and `KRONN_HOST_HOME` to
process-owned temporary fixtures. MCP mutations call the host-sync adapters, and
their target resolution prefers `KRONN_HOST_HOME`; setting only the data root
can therefore leave host-agent configuration paths unconfined.

The regression test supplies a plausible fallback home containing a sentinel,
runs project and host synchronization, and verifies that the adapters write
only below the owned fixture before restoring the previous environment.

[src: file: backend/tests/api_tests.rs:1641-1656]
[src: file: backend/src/core/mcp_scanner.rs:2494-2510]
[src: file: backend/src/core/mcp_scanner.rs:5231-5383]

# The audit validation did not stall in the matrix: its agent never started

`POST /api/projects/:id/validate-audit` refuses with « The validation discussion
has not finished — the agent must end on KRONN:VALIDATION_COMPLETE, or every
Critical and High tech-debt sheet must be settled » until the validation agent
has ended its message on `KRONN:VALIDATION_COMPLETE` (or every Critical and High
TD sheet is settled). The agent opens the discussion detached, right after the
`validation_created` event of the run, and it may ask the human to arbitrate: a
script driving the validation answers in the discussion
(`POST /api/discussions/:id/messages`) and waits for the agent to go idle again.

18/10 evening matrix, apollo `validate` cell (`agent_messages: 0` in its result):
nothing waited for a human and nothing stalled. Two bench causes, in the order
they hit:

1. **The agent could not start.** The backend log of the cell reads
   `Agent start failed: acp_start_failed … Claude ACP internal bridge is
   unavailable`. A discussion agent mounts Kronn's `kronn-internal` bridge; the
   bridge script (`scripts/disc-introspection-mcp.py`) is looked for next to the
   binary, in the source checkout it runs from, or at `KRONN_DISC_INTROSPECTION_MCP`
   / `KRONN_INTERNAL_MCP_EXECUTABLE`. The bench ran a copied binary from another
   directory with none of those, so only the audit steps (which mount no bridge)
   worked. Fix in the bench: export `KRONN_DISC_INTROSPECTION_MCP` to the
   checkout's script for the backend it starts.
2. **Its replies were refused.** The bench then sent
   `{"content": "...", "channel": "Main"}` three times and got
   `422 unknown variant 'Main', expected 'main' or 'note'`: the channel is
   `snake_case` on the wire (`matrix/matrix.py`, `'channel': 'main'`). The wire
   format is pinned by `channel_wire_tests` in `backend/src/models/discussions.rs`.

The cell read « not busy, no agent message » as the agent being done, and the
final `validate-audit` call as a stall. A discussion whose agent failed to start
says so in its own thread (« Claude Code n'a pas pu s'initialiser … repair the
Kronn installation or check the configured bridge executable/script »).

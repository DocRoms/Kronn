# Three bridge tests cannot run inside a Kronn agent session

## Symptom

`python3 -m unittest test_disc_introspection_mcp` run from a shell that a
Kronn room agent owns (Claude Code or Codex dispatched by Kronn) reports three
errors that CI (`test-python`) and a plain terminal do not have:

- `test_stdio_process_survives_real_reexec_and_serves_fresh_contract`
- `test_real_reexec_restores_fragmented_json_rpc_line`
- `test_real_reexec_preserves_queued_mutation_cancellation`

[src: file: backend/scripts/test_disc_introspection_mcp.py:9170-9338#test_stdio_process_survives_real_reexec_and_serves_fresh_contract]

## Cause

They re-execute the bridge as a real subprocess, which the agent sandbox
refuses (`setpgid: Operation not permitted`), so the child never answers and
the first `json.loads` on its stdout fails. The failure is identical on `main`.
Do not "fix" them: they are green in CI.

## What the suite already handles

Such a shell also exports `KRONN_*` (backend URL, bridge token, discussion id,
room-agent context), which the bridge reads when a test loads it. The module
fixture clears them for the whole run, so the rest of the suite does not
inherit the session's identity [src: file: backend/scripts/test_disc_introspection_mcp.py:59].

The Rust suite has a similar leak: run it with `TMPDIR=/tmp` and
`CLAUDE_CODE_DISABLE_AUTO_MEMORY` unset.

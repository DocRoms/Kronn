# Native MCP and file delivery qualification — 2026-10-02

KT-953 and KT-954 were reviewed from the unfinished CLI/file changes and tested
against isolated backends. The qualification covers actual MCP calls, durable
file delivery and the original discussion's browser behavior. It does not
establish operating-system isolation of Kronn's database or credentials.

## Actual CLI calls

| CLI | Runtime | Observation |
|---|---|---|
| Codex 0.159.3 | `ad7b30a3` | Real `disc_get_message` and `disc_append` succeeded with `full_access=false`, then `true`. |
| OpenCode 1.18.33 | `a13c3104` | Both calls succeeded in a projectless discussion with `full_access=false`. |
| Claude Code 2.1.285 | `0dcadaf5` | Both calls succeeded with `full_access=false` after the announced quota reset. |
| Copilot CLI 1.0.80 | `ad7b30a3` | Provider refused the feature because enterprise/organization policy was required, before MCP qualification. |
| Gemini CLI | — | No installed `gemini` executable was available for this qualification. The successful HTTP Gemini audit is a different transport. |

Each Codex case left exactly one completed dispatch job. Actual Codex JSONL
contained MCP names, arguments and lifecycle states; the persisted transcript
kept named completed traces rather than `mcp_tool_call()` placeholders. With
full access enabled, Codex also created two text fixtures: one was attached by
MCP, the other was delivered by an absolute Markdown link and attached when the
reply was saved. Both attachment endpoints returned the expected bytes.

An earlier run on `be7b0c32` had successfully called MCP but also scheduled
additional answers to its own posts. A native runtime's session id was mistaken
for a joined CLI peer. `ad7b30a3` requires a durable joined author for the implicit
handoff while preserving explicit recipients; its fresh canary showed no extra
jobs. The earlier database remains stopped with its pending jobs preserved.
[src: commit: ad7b30a3]

OpenCode initially reported that Kronn tools were absent. Its ACP input and the
broker log showed the cause: the owned bridge was rejected by the project-server
filter in a projectless session. `a13c3104` reconstructs that reserved entry from
the runtime, keeps project declarations from replacing or duplicating it, and
registers it separately. The new real ACP `session/new` contained
`kronn-internal`, followed by completed read and write tool updates.
[src: commit: a13c3104]

Claude Code initially exhausted its provider session quota on `ad7b30a3`.
A fresh isolated backend on `0dcadaf5`, after the announced reset, completed
both calls in 11.67 seconds without full access. The database contains the
exact MCP-posted sentinel, named completed read/write traces with observed
arguments, and exactly one completed dispatch job with one attempt. The backend
was stopped and its evidence frozen after the case.
[src: commit: 0dcadaf5]

## Files and browser behavior

The original discussion `5ad1d4d3-cd66-4699-8cd2-89706f29388e` was inspected in a
read-only browser session using the changed frontend against the user's running
backend. GIF and PNG opened in the existing image viewer, JSON opened as text,
and ZIP downloaded with the original filename and 683,867 bytes. This found two
review defects that were fixed before the final browser pass: a gallery could
return early when the selected attachment belonged to another message, and a
legacy ZIP with `text/plain` MIME opened as text instead of downloading.
[src: commit: be7b0c32]

Persistence tests cover attachment identity, credential/data-directory exclusions,
path and symlink containment, size/read budgets, Markdown rewriting, and cleanup
of uncommitted attachments. Frontend tests cover exact attachment links,
unavailable-file reasons, relative project paths and requested lines, and local
Markdown images. The complete frontend suite passed 5,129 tests, TypeScript 6
and 7 checks, both linters and locale checks. The file implementation on
`be7b0c32` passed the complete local Rust suite: 8,445 passed, 21 ignored, across
34 reported suites, plus formatting and Clippy on all targets.
[src: commit: be7b0c32]

The complete local suite on `ad7b30a3`, including native trace and routing
regressions, subsequently passed 8,453 tests with 21 ignored across 34 suites
in 1,154.19 seconds. These counts exclude separate targeted test commands.
The integrated revision `5f462a15`, including the ACP bridge and directory-failure
changes, passed 8,457 local Rust tests with 21 ignored across 34 suites in
1,003.96 seconds, plus formatting and all-target Clippy.
[src: commit: ad7b30a3] [src: commit: a13c3104] [src: commit: f315643e]
[src: commit: 5f462a15]

The first integrated Linux CI exposed a test-fixture dependency: the Codex
approval test used the optional `uvx` command, which the production availability
filter correctly omitted when absent. The fixture now uses the existing test
executable without executing it. All 95 MCP scanner tests and all-target Clippy
passed after that correction. The same CI's shell job stopped progressing during
the warm-backend test; the complete local shell suite passed 313 tests and an
isolated Linux reproduction passed the affected test 20 times. The cause of that
single CI stall was not reproduced; a fresh CI run is still required.
[src: commit: 4dc0de94]

A subsequent error-path review removed the silent shared-temp fallback when a
dedicated discussion directory cannot be created. The turn now fails before
agent launch, persists a visible System error, clears the untracked pending
reply and reports a preflight failure to the tracked caller. Regressions cover
an actual directory-creation failure and the persisted streaming refusal.
[src: commit: f315643e]

## Boundaries and evidence

Native traces correlate by call id, mask sensitive JSON keys and shared
credential patterns before truncation, and keep observed states. Missing runtime
metadata remains explicitly unknown. Tool output, MCP result bodies and patches
are not copied into the trace. Redaction is heuristic, not a proof that arbitrary
input cannot contain private information.
[src: file: backend/src/agents/tool_trace.rs:1]

The dedicated discussion folder and automatic-attachment filters are not a
filesystem sandbox. KT-953's separate criterion preventing a CLI from reading
Kronn's data directory is still unresolved. The release-room arbitration key
`release-0142-native-data-isolation` was pending when this report was written;
this report does not waive that criterion or close KT-953. Copilot and Gemini CLI
also remain unqualified for the reasons above.

Private canary reports, SSE, database copies and protocol captures remain under
`kronn-ab-bench/native-cli-20261002-traces-r1/` and
`kronn-ab-bench/native-opencode-20261002-acp-r1/`. The corresponding frozen binary
SHA-256 values are respectively
`bb9ef7340e3ff381492c6d6c1456a96a8db678ce3d4a423c642437b779660109`
and `61869f432fb0891c41f8c39b74be7436d4943b99f3c4177488e8996838d6b085`.
The final Claude evidence is in
`kronn-ab-bench/native-claude-20261002-final-r1/`, with binary SHA-256
`8d7f3ad104f244ea1a9bb79c9dcff025eed72ad9fb6274dbd07d7ac0cba5dd22`.
Failed pre-fix and provider-limited canaries are retained separately. Test
backends were stopped after their cases; the user's backend was not stopped.

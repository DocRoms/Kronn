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
| Claude Code 2.1.285 | `ad7b30a3` | Provider session quota stopped the turn before MCP qualification; the CLI announced a reset at 05:20 Europe/Paris. |
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
The following ACP bridge and directory-failure changes passed their targeted
regressions and all-target Clippy; the integrated revision is qualified by its
separate CI run.
[src: commit: ad7b30a3] [src: commit: a13c3104] [src: commit: f315643e]

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
this report does not waive that criterion or close KT-953. Claude, Copilot and
Gemini CLI also remain unqualified for the reasons above.

Private canary reports, SSE, database copies and protocol captures remain under
`kronn-ab-bench/native-cli-20261002-traces-r1/` and
`kronn-ab-bench/native-opencode-20261002-acp-r1/`. The corresponding frozen binary
SHA-256 values are respectively
`bb9ef7340e3ff381492c6d6c1456a96a8db678ce3d4a423c642437b779660109`
and `61869f432fb0891c41f8c39b74be7436d4943b99f3c4177488e8996838d6b085`.
Failed pre-fix and provider-limited canaries are retained separately. Test
backends were stopped after their cases; the user's backend was not stopped.

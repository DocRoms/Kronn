# Signal catalogue discovery, 2026-09-22

KT-679 ships a versioned catalogue through the existing `tool_manual(signals)`
entry point, with one shared backend registry for native and MCP callers. The
existing inline discussion and MCP instructions stay in production. Replacing
their schema with a short pointer lost the one successful proposal observed in
this bounded comparison: Gemma e2b's Quick Prompt card. The other five baseline
cells also failed, so this is not evidence of general reliability for either
prompt style.
[src: file: backend/src/api/signal_catalog.rs:1]
[src: file: docs/research/signal-catalogue-2026-09-22.json:1]

## Protocol and instrument correction

The corrected comparison used source `fe23675619fb640352fc756a37f08c3d9678427c` and frozen libtest
SHA-256 `fd539c0374f15325288dbc10ca9a63b25221db6f5fa22d49838bb78ee8ead2f3`, with `KRONN_SIGNAL_BENCH=1`, `KRONN_TIERED_TOOLS=0`,
`KRONN_OLLAMA_NUM_CTX_CAP=24576`, and each explicit `KRONN_BENCH_MODEL`.
Run the ignored test
`api::agent_tools::signal_bench::bench_native_signal_discovery`; set
`KRONN_BENCH_REPORT` to preserve its JSON observations. The host campaign ran
from 2026-09-22T14:05:02.331826+00:00 to 2026-09-22T14:06:43.848765+00:00. Both processes exited 101 because the final
assertion requires all cells to succeed; all twelve cells completed.
The initial sandbox attempt could not reach localhost Ollama and produced no
inference observations. No compilation or suite started by this agent overlapped
the corrected host campaign; the reviewer had reported its heavy suites ended.
Other host activity was not controlled.
[src: file: backend/src/api/agent_signal_bench.rs:1]
[src: file: docs/research/signal-catalogue-2026-09-22.json:1]

Both modes use the same full 59-tool native catalogue (42,775 serialized bytes),
including the new manual entry, real native handlers and a temporary database.
Only the French proposal notice differs: the existing 666-byte inline version
and an experimental 245-byte discovery pointer. This exercises the production
runner with an isolated notice and request, not a complete real discussion
history. The QP, QA and Workflow fixtures are real saved resources; their generated
IDs and timestamps differ between cells. The within-pair order is reversed for
QA. These are single observations, not randomized latency trials.
[src: file: backend/src/api/agent_signal_bench.rs:1]
[src: file: docs/research/signal-catalogue-2026-09-22.json:1]

A guarded executor exposes the full catalogue, forwards the permitted reads to
the real handler, and refuses mutations. Success requires target discovery,
exactly one durable proposed card of the right kind and ID, the requested QP
value with agent-suggestion provenance, no rejected call and zero launches.
The discovery mode also requires reading `tool_manual(signals)`. The response
is passed unchanged through the actual message transaction and action parser;
Markdown prose or a `json` fence is never repaired into a successful card.
[src: file: backend/src/api/agent_signal_bench.rs:1]

The first instrument incorrectly refused three legitimate exploratory reads:
`mcp_list`, `git_status` and `workflow_step_schema`. Its raw twelve observations
remain in `signal-catalogue-2026-09-22-attempt-1.json`, labelled with that limit.
The corrected guard forwards these reads; a self-test compares their outcomes
with the unwrapped executor and verifies that a QP launch still creates no job.
In the corrected campaign the only rejected calls were two `qa_run` attempts,
both from Qwen's inline-mode cells. These are retained as proposal-only failures.
Other calls outside the explicit read allowlist would still be refused; no
claim of unrestricted agent-task performance is made.
[src: file: backend/src/api/agent_signal_bench.rs:1]
[src: file: docs/research/signal-catalogue-2026-09-22-attempt-1.json:1]
[src: file: docs/research/signal-catalogue-2026-09-22.json:1]

## Corrected observations

| Model | Target | Notice | Result | First prompt tokens | All prompt tokens | Turns | Seconds |
| --- | --- | --- | --- | ---: | ---: | ---: | ---: |
| qwen3.5:2b | quick_prompt | inline | fail | 12425 | 50067 | 4 | 6.66 |
| qwen3.5:2b | quick_prompt | on demand | fail | 12316 | 25118 | 2 | 1.53 |
| qwen3.5:2b | quick_api | on demand | fail | 12312 | 50987 | 4 | 2.68 |
| qwen3.5:2b | quick_api | inline | fail | 12421 | 62869 | 5 | 4.07 |
| qwen3.5:2b | workflow | inline | fail | 12419 | 37462 | 3 | 2.64 |
| qwen3.5:2b | workflow | on demand | fail | 12310 | 51137 | 4 | 3.09 |
| gemma4:e2b | quick_prompt | inline | pass | 11764 | 23584 | 2 | 14.28 |
| gemma4:e2b | quick_prompt | on demand | fail | 11651 | 11651 | 1 | 9.76 |
| gemma4:e2b | quick_api | on demand | fail | 11646 | 11646 | 1 | 11.51 |
| gemma4:e2b | quick_api | inline | fail | 11759 | 23599 | 2 | 14.83 |
| gemma4:e2b | workflow | inline | fail | 11757 | 23664 | 2 | 16.90 |
| gemma4:e2b | workflow | on demand | fail | 11644 | 11644 | 1 | 12.25 |

Qwen read the new catalogue successfully in all three discovery cells, but
produced no valid card. Gemma did not read it in any discovery cell and invented
unsupported JSON fields. Its inline Quick Prompt cell did persist the requested
card; its inline QA and Workflow cells did not. No automation ran in any cell.
[src: file: docs/research/signal-catalogue-2026-09-22.json:1]

The rejected short notice saved 109 reported first-request tokens on Qwen and
113 on Gemma. Those savings are not shipped, and lower totals for unsuccessful
one-turn replies are not a useful efficiency gain. The MCP declaration surface
is 111 tools and 86,609 bytes, nine bytes below the preceding catalogue; no new
tool declaration was added. The unchanged context bootstrap remains within its
existing budget. No net workflow-token or latency improvement is claimed.
[src: file: docs/research/signal-catalogue-2026-09-22.json:1]
[src: file: backend/scripts/ci/mcp_surface_budget.py:1]

## Functional checks and limits

The catalogue's four advertised families (including QE) are exercised through
real HTTP message ingestion, remain proposed, and create no launch. Native and
MCP manual dispatch share the registry. Existing invalid-payload, idempotence,
secret provenance, project inheritance and historical terminal-signal checks
pass. The full schema survives production history clamping under a 32,768-token
cap. Discovery covers Automation cards in version 1; historical Planning, audit
and QP-improvement parsers remain separate.
[src: file: backend/tests/api_tests.rs:856]
[src: file: backend/src/api/agent_quick_prompt_tests.rs:1]
[src: file: backend/scripts/test_disc_introspection_mcp.py:11087]
[src: file: backend/src/agents/runner_test.rs:1480]
[src: file: backend/src/api/signal_catalog.rs:1]

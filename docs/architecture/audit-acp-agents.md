# Auditing with an ACP agent (KT-927)

An audit step can run on OpenCode, Gemini CLI, GitHub Copilot CLI, Kiro — the
native ACP agents — and on Claude Code and Codex through their adapters. All of
them run inside Kronn's ACP host, and the audit pipelines (Full and partial)
start them through one launcher
[src: file: backend/src/api/audit/agent_launch.rs:84-104#AuditAgentLauncher]. Three things went wrong there
on the first real bench (OpenCode on Ollama, `front_apollo`), and were one
assumption each: that the process `start` returns is the agent, that a refused
read is a failed tool, and that usage is something only Claude's stream-json
carries. This page records what replaced them.

The partial audit is covered by each section. It used to have none of the three
behaviours: no cancel path at all, for any agent, and no per-step metrics.

## Reading environment files

An environment file named like a template — `.env.dist`, `.env.example`,
`.env.sample`, `.env.template`, and the same with a qualifier in front
(`.env.local.dist`) — holds names and placeholder values and is committed on
purpose. A real one (`.env`, `.env.local`, `.envrc`, `prod.env`) or a key file
(`*.pem`, `*.key`, `id_rsa*`, `id_ed25519*`, anything under `.ssh/` or `.aws/`)
does not. The rule is one function, `is_secret_file`
[src: file: backend/src/acp/secret_files.rs:51-79#is_secret_file].

It is applied twice, because the two kinds of runtime are not alike.

**OpenCode asks without naming the file.** OpenCode's default `read` policy is
`ask` for `*.env` and `*.env.*` and `allow` for `*.env.example` only
(verified in OpenCode 1.18.33's bundled defaults). `.env.dist` matches `*.env.*`,
so OpenCode sent `session/request_permission` — with `kind: read` and no path in
`locations`, because the `read` tool asks with empty metadata and the pattern it
matched never reaches the request. The broker cannot tell `.env` from
`.env.dist` there, so it failed closed and answered with the reject option, and
OpenCode treats a rejection as the end of the turn: `RejectedError` sets the
session loop's `blocked` flag unless `experimental.continue_loop_on_deny` is on.
One refused read therefore ended the whole audit step.

The decision has to be made inside OpenCode, by pattern, before any question is
asked. Kronn starts `opencode acp` with an inline configuration
(`OPENCODE_CONFIG_CONTENT`, merged after the user's and the project's own files)
[src: file: backend/src/acp.rs:881-894#apply_opencode_policy]:

- `permission.read` denies `*.env` and `*.env.*`, then allows the four template
  suffixes (`*.env.<suffix>` and `*.env.*.<suffix>`). OpenCode honours the last
  matching rule, in the order written, so the allows follow the deny — a test
  pins that order [src: file: backend/src/acp/secret_files.rs:232-266#the_opencode_config_denies_real_env_files_and_allows_templates_in_that_order]. A denial is not a
  question put to Kronn: OpenCode hands its model an error as the tool's answer
  and the turn goes on.
- `experimental.continue_loop_on_deny` is `true`, so a refusal the broker does
  give (a path outside the project) no longer ends the turn either.

An operator who already passes their own `OPENCODE_CONFIG_CONTENT` keeps it
untouched; Kronn logs that it did not apply its policy.

**Other native ACP agents report the path.** For them the broker decides: a
`read`, `search`, `fetch` or `think` tool call whose `locations` name a secret
file is refused — under `full_access` too, because `full_access` widens what an
agent may do, never what it may read of a secret. A path that resolves to a
secret file through a link (`notes.txt` → `.env`) is refused under its innocent
name. Writes are the `full_access` gate's business and are unchanged
[src: file: backend/src/acp/permission_broker.rs:570-585#secret_target] [src: file: backend/src/acp/permission_broker.rs:768-776#location_is_secret].

A refusal is always an answer: the broker replies with the agent's own reject
option (`selected`, not `cancelled`), which an agent reads as the tool's result.
Whether it then carries on is the agent's decision; for OpenCode, the
configuration above makes it carry on.

## Stopping an audit

`cancel_audit` used to kill `tracker.running_pids[project]`. For an ACP agent
that PID belongs to a lifeline process (`sh -c 'read …'`) that only exists so
the agent has something to wait on; the agent itself runs in a session inside a
process the ACP host owns. Killing the lifeline made the pipeline believe the
step had ended, while OpenCode kept reading and writing: an effective stop took
about eight minutes.

Now an ACP agent is stopped like an HTTP agent (KT-924), through a token:

1. Both pipelines register a cancellation token per step **before** the agent
   starts (the handshake and a cold model's first answer can take long), and
   register a PID only for a direct CLI agent. `stops_with_token()` says which
   [src: file: backend/src/api/audit/agent_launch.rs:209-217#stops_with_token].
2. `cancel_audit` trips the token. The ACP session writes `session/cancel`
   (bounded to five seconds, so a wedged agent cannot hold the stop hostage),
   drops the turn, and shuts the agent's process down
   [src: file: backend/src/agents/runner.rs:5246-5248#ACP_CANCEL_GRACE] [src: file: backend/src/agents/runner.rs:5698-5717#task_cancel] [src: file: backend/src/agents/runner.rs:5772-5778#shutdown].
3. The process is started as its own process group, and shutdown kills the
   group, so what the agent started — a shell command, an MCP server — stops
   with it [src: file: backend/src/acp.rs:991-995#process_group] [src: file: backend/src/acp.rs:780-793#kill_process_group] [src: file: backend/src/acp.rs:1795-1803#shutdown].
4. The pipeline waits on the lifeline, which exits only after that shutdown, so
   the audit is finalised as `Cancelled` once the agent is really gone. This is
   also why the lifeline's PID is **not** killed: doing it ended the wait early
   [src: file: backend/src/agents/runner.rs:5780-5783#lifeline_stdin].

The partial audit now checks the stop between two steps and after each step's
agent, finalises the run as `Cancelled` and releases the project lease, like the
Full audit [src: file: backend/src/api/audit/drift.rs:785-812#mark_cancelled] [src: file: backend/src/api/audit/drift.rs:1211-1272#mark_cancelled_at_step]. A partial refresh that
was cancelled does not refresh the baseline, so its sections stay reported as
stale.

An agent forced onto its direct CLI (`KRONN_ACP_ADAPTER_CLAUDE=0`,
`KRONN_ACP_ADAPTER_CODEX=0`) is still stopped by its PID, which is then the
agent's own.

## Counting tokens

The step counter only read Claude's stream-json (`parse_claude_stream_line`).
Every ACP step was recorded at 0 tokens, per step and per run. Now:

- **Where the figure comes from.** Claude's stream-json is read as before
  (largest reading, because Claude's usage is cumulative per call). Any agent
  that streams text reports through the process's own usage counters: an ACP
  runtime puts them on the `session/prompt` response — OpenCode sends
  `inputTokens`, `outputTokens`, `cachedReadTokens`, `cachedWriteTokens`, which
  the ACP host now keeps apart instead of dropping the cache
  [src: file: backend/src/acp.rs:1394-1417#usage_from_prompt_result]. The counters are read as the stream goes and
  once more when it ends, because ACP reports them with the end of the turn.
- **What is recorded.** `audit_run_steps` keeps `input_tokens`, `output_tokens`,
  `cache_read_tokens` and `cache_write_tokens` (migration 205), and
  `step_tokens` stays the headline figure, input plus output, as before. Input is
  as the runtime states it; whether it already contains the cached share depends
  on the agent, as in `core::pricing::TokenCounters::from_agent_report`.
  `reasoning` tokens OpenCode reports as `thoughtTokens` are not added to the
  output: the figure is the runtime's own `outputTokens`.
- **Unknown is not zero.** A runtime that reports nothing — or a usage block that
  counts nothing — leaves every figure `NULL` and the `step_done` event's
  `tokens` and `total_tokens` `null`; a cache figure the runtime did not give is
  absent, not 0. The run total is unknown until a step has reported, then the
  sum of the steps that did: a lower bound when a later step is unknown. A step
  that never started an agent (its target could not be read, the agent failed to
  start) still says 0, because nothing was consumed.
- **Surfaces.** The recap panel already printed a missing figure as `—`; the
  project card now clears the last-step chip on a `null` instead of leaving the
  previous step's figure there.

The partial audit records the same thing per step (`step_done` carries `tokens`,
`duration_ms` and `total_tokens`, and `audit_run_steps` gets a row), which it did
not before.

## Cost and model (KT-997)

- **Where the cost comes from.** Only what a runtime reports itself: Claude
  Code's `total_cost_usd` (its stream-json `result` line, or the adapter's
  `Cost` event) and OpenRouter's `usage.cost`, summed per response
  [src: file: backend/src/agents/chat_codec.rs:151-160#cost_usd_micros] [src: file: backend/src/agents/runner.rs:14031-14037#total_cost_usd] [src: file: backend/src/agents/runner.rs:5673-5675#usd_micros] [src: file: backend/src/agents/runner.rs:9804-9810#cost_usd_micros].
  The recorded cost is never recomputed from a rate table; a finished step
  whose agent reported none is priced on read by `core::pricing` for the served
  model and shown as an estimate, never as a reported cost
  [src: file: backend/src/db/audit_runs.rs:1611-1657#price_steps].
- **Per step.** `audit_run_steps.cost_usd_micros` (integer micro-USD) sums the
  step's attempts. It stays `NULL` when any attempt that ran reported no cost:
  a sum over a silent attempt would be a floor shown as the whole. A reported
  zero is recorded as 0. `step_done` carries the same figure as
  `cost_usd_micros` (`null` when unknown)
  [src: file: backend/src/api/audit/agent_launch.rs:492-519#StepCost].
- **The run's model.** `audit_runs.model` is the model(s) the runtime reported
  serving, from the launch's provenance capture shared by every step (`a / b`
  when several). With none observed it is the model Settings or the named
  connection configure for the tier, suffixed ` (configured)`; with neither it
  stays `NULL`. The orchestration's `ServedModelRecorder` is not reused: it
  writes to a worker dispatch row.
- **The timeline's total.** Summed over the finished steps the timeline shows
  (the newest result of each step, carried ones included): `~x $` when every
  one has a cost, `≥ x $ (n steps unknown)` when some have none, `cost ?` when
  none has; a step counted with its estimate marks the total estimated (`≈`). A
  step still running counts for nothing yet
  [src: file: frontend/src/lib/audit-cost.ts:26-49#summarizeAuditCost].

## Activity without text (KT-950)

- **One probe, both channels.** An HTTP agent writes its last tool and call
  count on its run's usage; an ACP agent on the activity sink, which now counts
  the calls where they pass (`AgentActivity.calls`) so a reader polling it misses
  none. `AuditActivityProbe` reads either
  [src: file: backend/src/api/audit/agent_launch.rs:357-386#AuditActivityProbe].
- **Ticks, not lines.** For an agent without stream-json, both pipelines look at
  the probe on every line and every `ACTIVITY_TICK` (1 s): a moved tool sends
  `tool_call` with `calls`, moved tokens send `step_progress`, and both update the
  tracker the progress poll reads. A provider that is only thinking moves
  nothing. The tick does not push back the Full audit's 60 s zombie deadline.
- **Keep-alive.** A detached audit stream silent for 15 s sends an SSE comment
  (`: keep-alive`) [src: file: backend/src/core/sse_limits.rs:50#HEARTBEAT_INTERVAL] [src: file: backend/src/api/audit/mod.rs:74-90#detach_sse_stream]. A comment is not an
  event: no client handler sees it, so it holds the connection without passing
  for model activity.

## Version sources (KT-1000)

The tech-debt step and the security pass judge versions from dated sources
only. `VersionContext::load` reads the stored dependency watch (never
refreshed: no network) and the runtime pins of the repository, then picks the
live source with `select_version_source`: context7, else web, else the
watch. What an agent reaches comes from `version_reach`:

| Launch | context7 | Web |
|---|---|---|
| ACP session (default route of Claude, Codex, OpenCode…) | stdio `context7` server in `.mcp.json` | not claimed |
| Direct Claude CLI (`DirectCliMigration`) | any `context7` entry in the `.mcp.json` it is handed | yes, with full access |
| HTTP agent (Ollama, LiteLLM…) | no | no (`AUDIT_TOOLS` has no `web_fetch`) |
| Other direct CLIs | not claimed | not claimed |

The broker auto-approves `fetch`-kind tool calls, so an ACP runtime may well
reach the web; Kronn has no declaration of which runtimes expose such a tool,
so it does not claim it. A `[src: deps: …]` citation is checked against the
injected watch by the enforce gate (`lint_step_file_with_deps`).
`[src: file: backend/src/api/audit/agent_launch.rs:320-351#version_reach]`

## What this does not cover

- `full_audit`'s and `partial_audit`'s start of a **direct CLI** agent is
  unchanged.
- OpenCode is stopped and policed from outside the process. Kronn does not edit
  the user's or the project's OpenCode configuration.
- Whether an ACP runtime other than OpenCode continues after a refusal is the
  runtime's own behaviour; only OpenCode's `continue_loop_on_deny` is known and
  set.
- The partial audit emits `step_progress` only for an agent without stream-json
  (on its ticks); a Claude stream-json step's figure arrives with `step_done`.

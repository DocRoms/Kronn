# Worker review rounds — the limit, where it comes from, how to raise it

**Status:** in force (2026-10-17) · **Origin:** KT-1345. Single-task launches made through MCP exposed no
way to set the limit, so every principal-driven lane was capped at the SQL default of 3 and escalated
(then needed a manual `task_exec_reassign`) even when the remaining findings were real bugs to fix.

## The rule

A task execution gets `max_review_rounds` rounds of `request_changes`. `N` allows `N` re-offers and
escalates on the `N+1`-th (`new_rounds > max_review_rounds`).
`[src: file: backend/src/db/orchestration.rs:3986-4082#commit_review_checkpoint]`

- **Integer:** 1 to 1000. `0` is refused at every boundary.
  `[src: file: backend/src/models/setup.rs:158-179#parse_review_rounds_arg]`
- **Unlimited:** no cap. It is stored in the existing `NOT NULL INTEGER` columns as a **sentinel**,
  `UNLIMITED_REVIEW_ROUNDS = u32::MAX` (4294967295) — never `0`, so the `> max` comparison simply never
  fires and no migration is needed. APIs and the UI show it as `unlimited` / `∞`.
  `[src: file: backend/src/models/setup.rs:157#UNLIMITED_REVIEW_ROUNDS]`

Unlimited removes the **round** cap only. The escalations for real stalls (no delivery, watchdog,
quota, integration failures) are unchanged. What it does not bound is a worker looping on a review that
never converges: that burns tokens until one of those, or the token budget below, stops it. Settings and
the extend form say so next to the checkbox.

### The token budget stays authoritative

A campaign's `token_budget` (`NULL` = none; single-task launches have none) is checked, against the tokens
its executions' messages already consumed, before **every additional worker attempt**, not only when
picking new tasks. `execution_token_budget_stop` is the one check;
`[src: file: backend/src/db/orchestration.rs:1359#execution_token_budget_stop]`
three places use it, whatever the round limit (including unlimited):

- **`request_changes`:** inside the review checkpoint transaction. A spent budget parks the execution
  in `Escalated` (journal `escalated`, `reason: token_budget_exhausted`, with `tokens_used` and
  `token_budget`), posts a principal notice that names the budget as the cause, and schedules neither the
  joined-CLI re-offer nor the native dispatch; the attempt does not advance.
- **Extending an execution:** the limit is still raised and journaled (with the stop reason), but the
  execution is not re-offered. The answer carries `reoffered_to_worker: false`,
  `stop_reason: "token_budget_exhausted"` and `token_budget`.
- **`task_exec_reassign` with a `worker`** (CLI and native): refused with `token_budget_exhausted`.

The budget is not reset from here: the execution stays `Escalated` for a human to take over or abandon.

## The default — Settings > Workers

`[workers]` in `config.toml` (`WorkersConfig`): `default_max_review_rounds` (default **5**, min 1) and
`review_rounds_unlimited` (default `false`). Two fields rather than an optional integer so that unticking
« Unlimited » gives the previous number back. An older `config.toml` without `[workers]` loads with the
defaults. `[src: file: backend/src/models/setup.rs:100-110#WorkersConfig]`

| Surface | How |
|---|---|
| UI | Settings > Workers: current value, where it applies, number field, « Unlimited » checkbox with the token-burn warning |
| API | `GET` / `POST /api/config/workers` (`POST` validates: 400 `validation` and the saved value is kept) `[src: file: backend/src/api/setup.rs:759-783#save_workers_config]` |
| MCP | `task_exec_prepare` answers `default_max_review_rounds` (an integer or `"unlimited"`); `task_exec_status` shows `review_rounds` as `used/limit` |

## Who uses it

| Launch path | Behaviour |
|---|---|
| Campaign dialog (`TaskLaunchDialog`) | pre-filled from the setting, editable per run, `unlimited` checkbox with the same warning; an existing campaign shows its own. While the setting is loading, or if reading it fails, the field stays empty and the dialog **omits** the limit (the server then applies the setting); it never substitutes a built-in figure, and a value the human typed is always sent |
| `POST /api/orchestration/campaigns` | `max_review_rounds` omitted → the setting; an integer or `"unlimited"` otherwise; anything else is a 400 |
| `task_exec_prepare` / `task_exec_launch` (MCP, native tool, REST) | optional `max_review_rounds` (integer or `"unlimited"`); omitted → the setting `[src: file: backend/src/api/orchestration.rs:1737-1762#provision_single_task_execution_with_options]` |
| `DelegateSubtasks` workflow step | its own `max_review_rounds` (1-10) when set, else the setting `[src: file: backend/src/workflows/delegate_subtasks_step.rs:621]` |

The value is read **at launch** and copied onto the run and the execution: changing the setting never
touches an execution that exists.

### Unknown arguments are refused

`target_ref` used to be ignored by `task_exec_launch`, which silently launched from `main`. Both principal
channels now refuse an argument the tool does not declare, naming it and listing the accepted ones:
the Python bridge checks against its own `inputSchema` and the native tool layer against its catalogue.
`[src: file: backend/scripts/disc-introspection-mcp.py:6082#_refuse_unknown_arguments]`
`[src: file: backend/src/api/agent_tools.rs:1380#unknown_argument_refusal]`
Note that `task_exec_prepare` does not declare `base_rev` / `idempotency_key`: those belong to the launch.

**Absent is not `null`.** Omitting `max_review_rounds` takes the setting; an explicit `null` is refused
like any other non-integer, never defaulted. The Python bridge forwards the key when it is present (even as
`null`) and the REST bodies (campaign, prepare, launch) keep the difference when parsing, so all three
channels give the same answer as the native tool.

## Raising the limit of an existing execution

`task_exec_reassign({task_execution_id, extend_review_rounds, reason})` — rounds to **add**, or
`"unlimited"` — is the third change of that tool (ONE change per call, with `worker` and `validations`).
The browser's « Extend rounds » button on the execution card calls
`POST /api/orchestration/executions/{id}/extend-review-rounds` with the same body, journaled as a human.

- **Who can:** the execution's principal or a human, never its worker. The `source_agent` /
  `source_session_id` in a tool body are declared by the caller, so on the bridge route they are not enough:
  a bridge token must itself own the execution's parent room and not its child room (a worker's token owns
  the child room), otherwise the call is refused with a validation error. A call without a bridge token
  (operator, browser, loopback CLI) keeps the declared-principal check; the native tool resolves the caller
  from its dispatch.
  A token that owns both rooms (a principal that adopted the child room) is refused as well; that principal
  extends from a token owning only the parent room, or from the UI.
- The limit changes on that execution only, and the change is journaled (`review_rounds_extended`: actor,
  reason, previous and new limit). `[src: file: backend/src/db/orchestration.rs:6391#extend_review_rounds]`
- At the cap but still `AwaitingReview`, extending is all it takes: the next `request_changes` is
  accepted instead of escalating.
- An execution **the limit itself escalated** is also re-offered: the stored findings (the exhausted
  `request_changes` never delivered them) are posted to its worker's room, then it takes the ordinary
  reassignment to the *same* worker, unless the campaign's token budget is spent (see above). Only the
  LATEST entry into `Escalated` counts: a watchdog or quota stall after an older budget escalation is
  extended but not re-offered. `[src: file: backend/src/api/orchestration.rs:14886#extend_execution_review_rounds]`
- Refused once the execution is Done, Failed or Cancelled, when it already has no cap, and without a reason.

The escalation message posted to the principal names this option.

## Tool surface cost

Every declaration is paid for on every session. The new arguments are `{"type": ["integer", "string"]}`
with their contract in the `task_exec_prepare` / `task_exec_reassign` manuals; the bytes were found by
moving the return shape of `workflow_run_status` into its manual and tightening three descriptions, and
`CATALOGUE_MAX_BYTES` went **down** by 33 B (84,450 → 84,417).
`[src: file: backend/scripts/ci/mcp_surface_budget.py:166#CATALOGUE_MAX_BYTES]`

## Tests

Config round trip (integer, unlimited, invalid, older file), argument parsing, each launch path's default,
tool validation and unknown-argument refusal, `request_changes` beyond 3 at a limit of 5 and unlimited,
extend on an `AwaitingReview` and on an escalated execution, a spent token budget on `request_changes`,
extend and reassignment for a joined CLI and a native worker (unlimited rounds), the Python bridge, and vitest for the
Settings section, the launch dialog, the execution card and the extend form.

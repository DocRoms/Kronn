# HTTP-agent capabilities — what API-mode agents may and may not do

> Scope: the agents Kronn drives over an HTTP chat API rather than a local CLI process —
> `Ollama`, `LiteLlm`, `Nvidia` and `Custom` connections [src: file: backend/src/agents/runner.rs:3614-3619#is_http_chat_agent].
> This file is the boundary future capability requests are judged against (KT-338).

## The rule that governs everything Kronn injects

> **Toute aide comportementale ou modification de réglage doit démontrer un
> bénéfice sur plusieurs modèles et tâches représentatives avant activation.
> Les informations nécessaires à l'exactitude du protocole restent
> obligatoires, et leur présentation se mesure séparément.**

The line falls between the two: a consigne, a progress digest, a nudge or a
sampling knob is a behavioural aid and has to earn its place. A tool that
errored, a result that was shortened, a permission that was refused and a
schema that was corrected are protocol facts; withholding them would make the
run lie, so they are stated whatever they cost — what gets measured there is
how they are worded, not whether they are sent. This file already requires the
truncation notice for that reason.

A behavioural mechanism with no measured effect is not neutral. It moves the
model's behaviour, and the degradation is then blamed on the model. What that
looked like on 2026-09-21, on one job — an inventory of 20 files in a real
repository, on six local models: two mechanisms lowered the result, and two
others never had their trigger fire, so they were not judged at all. The
sharpest case took a model from 9 files read and 0 unsupported lines to 2 files
and 7, by doing nothing more than telling it, factually and correctly, which
files it had already read.

That is an observation on one task shape and one family of mechanisms, not a
law: nothing here shows that every form of progress information, in every
scenario, has that effect. What did help on that job removed obstacles and said
nothing — see
[what was tried and measured](../research/local-agent-mechanisms-2026-09-21.md),
which records each attempt, what it is evidence for, and what it is not.

How to comply: put the mechanism behind an env flag, default off; measure flag
off then flag on **in the same build** (changing a tool description moves every
measurement); ship it only if it improves a real metric on several models. If it
does not, do not commit it at all rather than leave it off "just in case".

A mechanism whose trigger never fired is **not evaluated**, not harmful: say so
rather than filing it with what was measured and rejected.

## Why a boundary exists at all

CLI agents run as a local process with the user's own login, a filesystem, a shell, and an
MCP bridge. HTTP agents have none of that: they are a request/response loop, and every
capability they appear to have is a **tool Kronn executes server-side on their behalf**.

The MCP bridge is deliberately not one of those capabilities. The HTTP path returns before
any MCP environment is built [src: file: backend/src/agents/runner.rs:4539-4560#start_ollama_http_with_idle], and
`--mcp-config` is a CLI-only argument [src: file: backend/src/agents/runner.rs:4807-4824#insert_claude_mcp_config].
So "add a tool to the MCP server" never reaches an HTTP agent. Anything they must be able to
do has to exist as a **native tool in Kronn's own catalogue**.

That constraint is the whole reason a boundary has to be written down rather than inferred:
each capability is a piece of Kronn, executed with Kronn's privileges, on behalf of a model
that may be hosted by a third party.

## What they have

The workspace catalogue, thirteen tools [src: file: backend/src/api/agent_workspace_tools.rs:1119-1133#TOOL_NAMES]:

| Tool | Purpose |
|---|---|
| `web_fetch` | Fetch one http(s) URL server-side |
| `read_file` | Read one file inside the workspace |
| `write_file` | Create or overwrite one file inside the workspace |
| `edit_file`, `edit_lines`, `insert_after_line` | Change part of an existing file, guarded by its content hash |
| `list_files` | List a directory, optionally recursive |
| `find_files` | Glob search inside the workspace |
| `search_text` | Search file contents for a literal string |
| `git_status`, `git_diff`, `git_log` | Read the workspace repository |
| `git_commit` | Commit named files, only for the active native worker in its task worktree |

Plus Kronn's internal catalogue — plan and task tools, `qa_list`/`qa_run`, and `api_call`
against configured REST plugins [src: file: backend/src/api/agent_tools.rs:606-1127#build_tool_catalogue].

And, since 0.13.0, media generation: `media_generate` and `media_job_status`
[src: file: backend/src/api/agent_tools.rs:1268-1363#orchestration_tool_catalogue]. These sit in the orchestration
catalogue, so an HTTP agent replying in a discussion has them, and a bounded worker keeps
them — they are deliberately absent from the principal-only list a worker room strips
[src: file: backend/src/api/agent_tools.rs:1620-1673#NOT_FOR_A_WORKER].

Since 0.15.3 a Workflow Agent step on an HTTP or local agent (Ollama, LiteLLM, OpenRouter…)
has these two tools as well, and **only** these two: every other orchestration tool
(`agent_list`, `task_exec_*`, `agent_job_start`, `agent_schedule_wake`…) stays out of its
catalogue and is refused when named
[src: file: backend/src/api/agent_tools.rs:1591-1618#workflow_tool_catalogue]. The step runs the
very same handler as a discussion — one body, so the validation, the request guard, the
connection resolution and the idempotency key cannot drift
[src: file: backend/src/api/agent_media_tools.rs:58-72#generate_media] — and differs only in where
the asset goes:

- **A room of the run.** A step has no discussion, so before its first generation Kronn opens
  one in the workflow's project and links it to the run (`discussions.workflow_run_id`: the run's
  discussions list it, deleting the run deletes it). The step's next generation reuses it, which
  is what lets a picture feed a clip within one step; a refusal that leaves it empty takes it
  back. Concurrent first calls of a run are serialised, so there is exactly one room and the
  caps below cannot be passed by calls launched together. The bridge gate adopts that room, so
  the step can read its own job and no other.
- **The run artifacts.** Once the job is `completed`, `media_job_status` copies the asset to
  `<run artifacts dir>/<job id>.<ext>` and returns `asset.artifact_path` beside the context file
  id. The directory is the one `{{run.artifacts_dir}}` names, so a later Exec step or an Agent step
  reading it read-only can use the file; it lives as long as the run
  [src: file: backend/src/workflows/run_artifacts.rs:259-271#keeps_directory]. The copy is written through
  the run directory's file descriptor (KT-1585), because an Exec step can write there: a link
  planted at `<job id>.<ext>` is replaced, never written through
  [src: file: backend/src/api/agent_media_tools.rs:1051-1081#copy_into_run].
- **Waiting.** A step has no room to wake, so `wake_when_ready` is neither declared nor accepted
  (it is refused before anything is billed). `media_job_status` takes `wait_secs` (capped at 300)
  and blocks until the job settles; an unsettled answer says `settled: false` and not to generate
  again. A video that outlasts the cap is not lost: the worker keeps going, the step calls again.
- **The billing guard.** The request the guard counts is the run and the step
  (`workflow-step:<run> · <step>`): at most six generations per step, no retry once one failed, no
  switch of connection or loss of references. A step that runs more than once in a run (a retry,
  a loop) shares that budget on purpose. On top of it a run may start at most 12 generations in
  all (`KRONN_WORKFLOW_MEDIA_JOBS_PER_RUN` changes it; a value above 100 is lowered to 100 and an
  unparsable or zero one falls back to 12), so a cron workflow is bounded per run.
  The Test button is its own request on every press, keyed on its project and a fresh id: two
  presses, two workflows or two projects never share a cap or a room. A **dry run** Test refuses
  `media_generate` before anything is queued, because a dry run only adds a preamble to the
  prompt and would otherwise bill.
- **References.** `reference_asset_ids` works in a step with `reference_mode`, resolved inside
  the run's own scope and brought into the step's room as a context file (once per source), where
  the discussion's own checks then apply. The accepted sources are, in this order: an image
  already in the step's room (id or filename); the context file id of a picture in a room of
  **this run** or in a discussion of **this project** (a brand image whose id a launch variable
  carries); the `job_id` of a completed generation of **this run** (an earlier step's image), or
  the `artifact_path` `media_job_status` returned for it; a png, jpg, webp or gif file **inside
  the project**, named relative to its root. That file is read through directory fds, never
  through a link: no `..`, no absolute path, a link anywhere on the path (directory or file) is
  refused, a path through a file (`a.png/x`) is refused as "not a directory", a FIFO or device
  is refused without blocking, a file over 12 MiB is refused even if it grew while being read,
  and it is a picture by its first bytes (PNG, JPEG, WebP, GIF), not by its name. A name that matches nothing gets the same single
  refusal as an id of another scope.
  Any discussion of the same project counts as a source, rooms shared with peers included: fine
  for one account, to be revisited with KT-1235 (multi-account).
  A job of another run, a human's own generation, another project's file and a path in another
  run's artifacts directory all get one refusal that does not say whether the id exists. An
  arbitrary file of the artifacts directory is not a source: an Exec step writes there, and
  only the copies Kronn made of generated assets are resolved, through their job.

Until then only CLI agents could generate an image or a video, because the tools were
declared in the MCP bridge and an HTTP agent structurally cannot reach it. The capability
existed and was announced to CLI agents by `kronn_intro`; the HTTP half of the fleet was
told about a feature it had no way to invoke.

And, since 0.15.0, the five Page tools: `page_list`, `page_get`, `page_create`,
`page_update_html` and `page_add_dataset`. They call the same handlers as the MCP bridge,
so validation, immutable revisions and inert action blocks are shared. A workflow Agent
step has them as well; its catalogue still refuses every other mutating tool. See
[Native Page tools](../operations/native-page-tools.md).
[src: file: backend/src/api/agent_page_tools.rs:18-40#declarations]

## The one capability that spends money

`media_generate` is different in kind from every other tool above, and the difference is
worth stating rather than discovering: **it bills**. A `read_file` that goes wrong costs
nothing; a video generation that goes wrong costs what the provider charges for it.

The bounds that make that acceptable are not in the tool, they are around it:

- The modality is **required**, never defaulted. An agent that omits it is refused rather
  than silently billed for the wrong kind of asset.
- The discussion is taken from the room the agent is speaking in, not from an argument. An
  agent cannot direct a generation — or its cost — at a room it is not in.
- A job is claimed once (`media_jobs` reuses the `agent_resume_jobs` claim), so a retried
  tool call does not buy the asset twice
  [src: file: docs/architecture/media-generation.md:80-81#agent_resume_jobs].
- An agent only learns a modality exists when a connection has a model configured for it —
  the worker catalogue lists modalities one per configured model and stays silent otherwise.
  An agent is never told it can produce a video that the request would then refuse.

Every one of these is bounded, and the bounds are part of the contract, not an
implementation detail:

- **Filesystem**: paths resolve inside the discussion's workspace and cannot leave it, by
  `..` or by symlink. The root is the discussion's `managed` workspace row (else its first
  declared row) when it has one, otherwise the path of the project the discussion belongs
  to; a workflow step has no discussion and resolves straight to its project's path
  [src: file: backend/src/api/agent_tools.rs:3420-3487#workspace_root]. The project fallback is the common
  case, not the exception: a workspace row is only created by orchestration, so on
  2026-08-19 just 16 of 395 discussions had one. Requiring such a row before granting the
  file tools was the original bug — the path was reachable all along.
- **Network**: `web_fetch` refuses private and loopback addresses before any request
  [src: file: backend/src/api/agent_workspace_tools.rs:268-296#check_fetch_url], times out at 20s
  [src: file: backend/src/api/agent_workspace_tools.rs:42-44#FETCH_TIMEOUT_SECS], and caps the body at 256 KB
  [src: file: backend/src/api/agent_workspace_tools.rs:38-40#MAX_BYTES].
- **Truncation is always announced.** A bounded read that returns part of a document sets a
  `truncated` flag, so a model can say it is reasoning from a partial view instead of
  concluding from one. Walks are capped at 20 000 entries and say so
  [src: file: backend/src/api/agent_workspace_tools.rs:52-57#MAX_WALK_ENTRIES].
- **Git is read-only, with one narrow exception.** Status, diff and log read the repository;
  `git_commit` commits explicitly named files, and only for the exact active native worker in
  its managed task worktree. Nothing amends, checks out or pushes.

## What they do not have, and why

- **No shell.** No arbitrary command execution, so no test runs, no build, no package
  manager. This is the load-bearing exclusion: a shell is a second runtime, and the CLI path
  already has one that works.
- **No mutating git outside a task execution.** No checkout, branch, merge or push; a commit
  only through `git_commit`, as above.
- **No MCP servers.** See above — structurally unreachable, not merely disabled.
- **No filesystem outside the workspace.** Including no absolute paths.

The consequence to state plainly: **an HTTP agent can change a file but cannot prove the
change works.** Verification — running the suite, reading the failure, committing once green
— stays with CLI agents. A workflow that hands implementation to an HTTP agent must route the
verification step to a CLI agent, or it is claiming a green it never measured.

## The boundary moved once, on purpose

KT-338 was originally scoped as *analysis only*: search, review, triage, synthesis, with
implementation reserved for CLI agents. On **2026-08-18** the user asked explicitly that
Ollama-class agents be able to *get and create files*, and `write_file` was added.

So the boundary today is **not** "HTTP agents cannot write". It is:

> HTTP agents may **read and write files** inside a bounded workspace and **read** its git
> history. They may not **execute** anything — no shell, no mutating git — and therefore
> cannot verify their own work.

One trap in that sentence, worth naming because the catalogue says otherwise: `git_commit`
IS declared to a discussion agent, and its handler refuses every caller that is not the
active native worker in its managed task worktree
`[src: file: backend/src/api/agent_tools.rs:3723-3771#git_commit]`. So an ordinary `@ollama` in a room
sees the tool and is turned away by it. The refusal is correct; the declaration is the part
that misleads, and it is kept declared only because a worker room is built by narrowing the
same catalogue.

Recording the move matters because the earlier wording still circulates in task descriptions
written before that date. Where a document says "implementation stays with CLI agents", read
it as "execution and verification stay with CLI agents".

It moved a second time on **2026-09-17**, for the automations. An HTTP agent could already
start a saved Quick Exec through `agent_job_start`, and had no tool that could tell it one
existed; it can now list them, run one synchronously, and author Quick APIs and Quick Execs
the way a CLI agent does. `qe_run` is execution, and it is deliberately the same narrow
shape as media generation: the human saved the command, Kronn owns the argv, there is no
shell, and the project scope is the one `start_background_job` already enforces. The agent
chooses which saved command to run — never what it does.

None of it reaches a worker. A worker has one task and is already briefed; authoring an
automation would change the instance in a way its delivery cannot be reviewed against, and
choosing among a visited project's saved commands is not part of its contract (KT-398).

## When a ceiling is reached, the human hears about it (KT-677)

A run is bounded: each tool has a call ceiling, the whole run has a round ceiling, and the
operator's configured duration sits above both. Those numbers used to speak to the model
only. The human received a partial answer with nothing saying a ceiling had cut it, which
one, at what value, or what was left out — and no way to say "continue".

Now Kronn says it itself, under the answer, whatever the model wrote: which ceiling, its
value, how many calls were refused after it, and what those calls were after (the first
five, so "what is missing" is concrete). The same message carries a `kronn-question` fence
with three options: more calls or rounds for this agent's next turn, no call limit for the
rest of the discussion, or keep the partial answer.

* **A decision only moves a counter.** Identical repeats, same-answer digests, the
  three-error circuit and the global timeout are not budgets: no grant, including an
  unlimited one, touches them. An unlimited answer is bounded to its discussion.
* **Only a question Kronn recorded can raise a budget.** `discussion_ceiling_requests`
  (migration 184) is written with the message that carries the fence. An agent writing a
  look-alike question in its own message grants itself nothing.
* **A grant is spent by one run**, the next one of the agent that hit the ceiling. Keeping
  the partial answer wakes nobody: that decision alone publishes no dispatch.
* **The round ceiling adapts to the model's window.** A 24K model cannot use 150 rounds and
  a 200K one doing an analysis can use more, so the ceiling follows the window Kronn knows:
  measured for a local model, published per endpoint for an OpenRouter one, and the
  long-standing 150 where the provider says nothing. A worker keeps the flat 150: its
  phases are bounded already and it has nobody to ask mid-delivery.
* **In a discussion, the round ceiling is no longer a failure.** The round's calls are
  refused, the model answers with what it has, and the question follows. A workflow step or
  a worker still fails with its reason: there is no one there to answer.
  [src: file: backend/src/agents/runner.rs:11285-11394#ask_on_ceiling]
  [src: file: backend/src/api/discussions/ceilings.rs:117-205#ceiling_question]

## Saved Quick Prompt generation settings (KT-670)

Single native `qp_run` launches preserve the saved agent, connection, tier,
model and bindings. Effort and output-token controls are captured in the same
transaction as the child discussion and its dispatch job. UI single-discussion
launches use the same capture helper. HTTP snapshots bind the discussion,
agent, connection, tier and resolved model; changing the target does not apply
the old target's controls. Existing discussions are not backfilled from a
mutable Quick Prompt. CLI effort keeps its existing snapshot format.
[src: file: backend/src/api/mcp_remote.rs:584-612#create_batch_run_with_launch_settings]
[src: file: backend/src/db/discussion_launch_settings.rs:18-117#capture]
[src: file: backend/src/db/sql/186_discussion_http_settings.sql:1-10#discussion_http_settings]
[src: file: backend/src/api/discussions/crud.rs:664-675#capture]

HTTP launches transmit explicit `reasoning_effort` and `max_tokens` on the
OpenAI-compatible wire. Ollama receives `think` and `options.num_predict` after
context fitting; an explicit thinking request removes Kronn's older Qwen
`/no_think` control message. Limits apply per provider request, including tool
round trips, rather than defining a total run budget. The provider may refuse
a setting its selected model does not support. Kronn does not retry by dropping
the saved control. [src: file: backend/src/agents/generation_settings.rs:39-71#apply_http]
[src: file: backend/src/agents/runner.rs:10038#apply_http]
[src: url: https://docs.ollama.com/api/chat]
[src: url: https://docs.ollama.com/capabilities/thinking]
[src: url: https://docs.litellm.ai/docs/completion/input]

CLI transports currently have no implemented `max_tokens` mapping. A single QP
launch with that explicit limit is refused before a child or job is committed,
with an instruction to clear the setting or choose an HTTP provider. An explicit
effort on an unsupported transport is also refused. The runner applies the same
validation to explicit workflow Agent-step controls. Legacy batch/compare target
overrides are outside this single-launch snapshot change.
[src: file: backend/src/api/agent_quick_prompt_tests.rs:591-644#quick_prompt_unsupported_saved_limit_refuses_without_a_child_or_dispatch_job]
[src: file: backend/src/agents/generation_settings.rs:5-28#validate]
[src: file: backend/src/workflows/steps.rs:1395-1399#reasoning_effort_override]
[src: file: backend/src/agents/runner.rs:4078-4082#validate]

The HTTP regression launches through the native QP tool and the discussion
stream, edits the template while the job is queued, then inspects both requests
received by a loopback provider. A separate Ollama fixture checks the actual
request after context fitting. These fixtures prove transport and persistence,
not live hosted-provider availability or model quality.
[src: file: backend/src/api/agent_quick_prompt_tests.rs:506-584#quick_prompt_saved_http_settings_reach_each_provider_turn_after_template_edit]
[src: file: backend/src/agents/generation_settings.rs:119-188#ollama_generation_controls_reach_the_actual_http_request_after_context_fitting]

## Full catalogue by default; progressive loading is experimental (KT-685)

HTTP discussion agents receive the full native catalogue by default. Explicitly
setting `KRONN_TIERED_TOOLS=1` starts with the core tools and an index of the
`pages`, `media`, `delegation`, `edit` and `automations` families. `tools_load` adds a family
for the remainder of the run. Bounded workers cannot expand their catalogue.
[src: file: backend/src/api/agent_tools.rs:428-508#TOOL_FAMILIES]
[src: file: backend/src/api/agent_tools.rs:2696-2704#tools_load]

The native dispatcher now routes `tools_load` correctly. Earlier benchmarks used
a substitute dispatcher and their claimed savings did not validate this path.
A new campaign used the real executor, seven scenarios and all six installed
local models: full declarations succeeded in 35/42 cells, progressive loading in
17/42, with regressions for every model. This does not establish a saving at
comparable quality. The full protocol, individual costs, failures and limitations
are in [the campaign report](../research/native-tool-catalogue-2026-09-22.md).
[src: file: docs/research/native-tool-catalogue-2026-09-22.json:44-2757#reports]

Tiering reduces the initial declaration text, not the context capacity required
to reach every family. Window reservation counts the whole reachable catalogue,
including JSON separators, before loading a family. A regression test checks
exact byte counts and an unchanged window through every family load.
[src: file: backend/src/agents/runner.rs:7295-7325#reachable_tools_bytes]
[src: file: backend/src/agents/runner_test.rs:1724-1762#the_window_does_not_move_when_a_run_loads_a_tool_family]

## Audits (KT-924)

An audit's only deliverable is files under `docs/`, so the gate
`agent_can_audit` admits an agent only if it can write there. It used to
refuse Ollama and LiteLLM on the premise that the HTTP path "has neither a
filesystem nor a tool loop". KT-338 had already made that false: the file tools
above are executed server-side. What was still missing was the scope — those
tools bound themselves to a discussion's workspace, and the audit pipeline passed
the agent no executor at all, so lifting the gate alone would have produced steps
that "succeed" without writing anything.

**Who is accepted.** Every agent but Vibe: the CLI agents (Claude Code, Codex,
OpenCode, Gemini CLI, Kiro, GitHub Copilot) and the HTTP agents Ollama, LiteLLM,
NVIDIA and Custom [src: file: backend/src/api/audit/mod.rs:750-765#agent_can_audit].
Vibe is refused because it has no file tools. Both launch endpoints (Full and
partial) share the gate and its refusal message, which lists the accepted agents
and is pinned to the gate by a test
[src: file: backend/src/api/audit/mod.rs:1580-1606#a_refused_agent_is_told_who_is_accepted_and_the_message_matches_the_gate].
A hosted HTTP agent (NVIDIA, a Custom endpoint) receives the repository content the
audit reads, so choosing one is choosing what leaves the machine.

**How an audit step reaches the project.** Both pipelines start the step's agent
through one launcher [src: file: backend/src/api/audit/agent_launch.rs:301-356#start]. A CLI
agent is spawned exactly as before. An HTTP agent gets a
`KronnToolExecutor::audit_arc_for_step`, scoped to the project directory the pipeline
already resolved (`resolve_host_path`, so it is right inside a container) rather
than a workspace row or a project looked up by id
[src: file: backend/src/api/agent_tools.rs:338-361#audit_arc_for_step]. The bounds are KT-338's, unchanged:
every path is canonicalised against that root and refused when it leaves it, by
`..`, by an absolute path or by a symlink; there is no shell; a truncated read
says so.

**What the audit's tools are.** Narrower than a workflow Agent step's:
`read_file`, `list_files`, `find_files`, `search_text`, `git_status`, `git_diff`,
`git_log` to read, and `write_file`, `edit_file`, `edit_lines`,
`insert_after_line` to write [src: file: backend/src/api/agent_tools.rs:379-396#AUDIT_TOOLS].
No `web_fetch` (it would send the model off the project), no `git_commit` (the
audit never commits), no plan, REST-plugin or discussion tools. A call to any
other name is refused before a handler sees it, and so is a write to the
target document of another step of the chain (the consolidation step excepted)
[src: file: backend/src/api/agent_tools.rs:1872-1886#AUDIT_TOOLS]
[src: file: backend/src/api/agent_tools.rs:279-311#foreign_step_target_refusal].

**What the audit prompt says about other repos (KT-926).** An audit's file tools
cannot leave the project, and the prompt goes to the model provider. It used to
list every other project registered in Kronn (name and absolute path), tell the
agent to read their `docs/AGENTS.md` and to write a `## Suggested companion
repos` section into the audited repository's own `docs/AGENTS.md` — asking an
HTTP agent for a read its tools refuse, and copying an unrelated project's name
into a versioned file. Neither pipeline injects that list any more, and no
discussion or workflow Agent step does either (the audit's validation and
briefing are discussions). The only
other repos a step names are those the user linked to the project
(`Project.linked_repos`), a voluntary declaration that stays in every step
[src: file: backend/src/api/audit/full.rs:337-338#format_linked_repos_for_prompt]
[src: file: backend/src/api/audit/drift.rs:468-469#format_linked_repos_for_prompt]. What remains: the block for a
linked repo still says to start with `<repo-path>/docs/AGENTS.md`, a path an HTTP
agent's tools refuse because it is outside the project, so for that agent the
block is information, not something it can follow. Making that block
transport-aware is not part of KT-926.

**A step that writes nothing is a failure.** This is not new machinery — it is
why the gates exist. An HTTP run that answers in prose instead of calling a tool
ends cleanly, so its exit code says "success"; the output validator and the
rewrite proof are what record the step as failed when its target is missing,
still the template, or byte-identical to before the agent ran
[src: file: backend/src/api/audit/validation.rs:225-357#validate_step_output]
[src: file: backend/src/api/audit/validation.rs:61-71#target_snapshot]. The final review step has no
target file and is judged on the exit code alone, for an HTTP agent as for a CLI
one.

**Stopping.** An HTTP agent's request and tool loop are a task, not a process:
killing the lifeline PID does not stop it. A Full audit therefore registers a
cancellation token per step, and `cancel_audit` trips it
[src: file: backend/src/api/audit/full.rs:1925-1930#agent_cancels]
[src: file: backend/src/api/audit/full.rs:4806-4809#agent_cancels]. Since KT-927 the partial audit does
the same, and so does an ACP agent, whose PID is a lifeline too: see
[auditing with an ACP agent](audit-acp-agents.md).

**What this does not change.** The per-tool call ceilings of a run still apply
(twelve `write_file` calls, for instance, before the tool is withdrawn), and an
audit has nobody to ask for more, so a step that hits one ends with what it has
and is judged by the gates. Whether those ceilings suit a step that writes many
files is a question for the bench this was built for, not something to tune here
unmeasured — see the rule at the top of this file.

## Asking for part of an API response (KT-929)

An HTTP agent's context window is fixed when its model loads, and a 200 KB API
answer does not fit a 32K one. Kronn shortens such a result before the next
request, which is a protocol fact and stays. What was missing was the other half
of the sentence the shortened result carried, "ask for a part you have not seen":
`api_call` had no argument to say which part. The broker already took an
`extract` (`AgentApiCallRequest.extract`, the workflows' `ExtractSpec`) and the
CLI bridge already declared it; the native declaration had no such argument and
its handler passed `extract: None`. The discussion that prompted KT-929 asked
about the latest Core Web Vitals on SpeedCurve: six successful `/v1/tests` calls,
each too big for the window, a model re-issuing the same or neighbouring calls
because it could not select anything, refusals, a forced end of the tool loop, and
an answer with LCP, FCP, INP and TTFB missing.

This is a capability and an information, not a behavioural aid, so the rule at
the top of this file does not ask for a measured benefit across models before
shipping it. Three things changed:

- **`api_call` takes `extract`.** An object `{"path": "<JSONPath>"}`, with the
  optional `fallback` and `fail_on_empty` of `ExtractSpec`; a bare JSONPath string
  is accepted as its `path`. An empty or malformed value is refused with a working
  example before anything is sent. The declaration says why: a large response is
  shortened and cannot be re-read whole
  [src: file: backend/src/api/agent_tools.rs:955-983#extract]
  [src: file: backend/src/api/agent_tools.rs:4886-4910#api_call_extract].
- **A shortened API result says what it holds.** Its `shape` lists the response's
  keys (small flat objects such as `meta` and `links` by value, so the paging is
  visible), the path and length of the main list, and the keys of one element down
  to its nested objects, up to 40 names per level with the count of those left
  out. The note gives `extract` examples built from that shape and checked against
  the original response (each is parsed and run; one that selects nothing is not
  offered), plus the path of one whole element to see every key. The shape is
  derived from the original result before any cutting, so a result shortened again
  on a later turn keeps it. A result that was already an extract gets no paths,
  which would not apply to the response the next `extract` runs on, and is told to
  select less. Other tools are shortened exactly as before
  [src: file: backend/src/agents/runner.rs:7551-7579#shape_of]
  [src: file: backend/src/agents/runner.rs:7581-7621#working_extract_examples].
- **A call that differs only by its `extract` or its query is not a repeat.** The
  duplicate guard compares the tool name and every argument, sorted by key, so an
  identical call is still replayed once and then refused, and a call with another
  `extract`, page or period runs [src: file: backend/src/agents/runner.rs:922-933#tool_call_signature].

Measured by the test that plays the whole exchange on a hundred runs of about 2 KB
each, with their metrics nested, the full native catalogue (51,990 B) and a
36,043 token window: the 198,119 B answer is shortened to 8,005 B, of which the
shape is 1,071 B; the model reads the shape, sends one call with
`$.data[*].metrics['lcp','fcp','inp','ttfb']`, and gets the 400 values in 2,127 B,
unshortened; the conversation then stands at 21,421 estimated tokens of 36,043
[src: file: backend/src/agents/runner_test.rs:2994#a_200_kb_response_yields_the_web_vitals_in_one_targeted_call_within_a_32k_window]
[src: file: backend/src/agents/runner_test.rs:3001#a_200_kb_response_yields_the_web_vitals].

**The window follows the catalogue (KT-1527).** The three shortening tests
(`a_shortened_api_response_announces_its_shape_and_how_to_ask_for_a_part`, the one
above, and `shortening_a_result_twice_keeps_its_note_and_shape_and_still_shrinks`)
used to sit about 100 B from the edge of a literal 32K window, so ~200 B of new tool
schema text pushed them onto the plain-text cut and they stopped being valid JSON.
They now size their window from the catalogue they carry
(`discussion_body_sized_to_catalogue`): 32,768 tokens at minimum, otherwise
`(declared catalogue + FIXTURE_MESSAGE_ROOM) / 2 + 2,048`, the same arithmetic as
`clamp_ollama_tool_results` (2 bytes a token, 2,048 tokens kept for the reply).
`FIXTURE_MESSAGE_ROOM` is 16 KB against the ~9.4 KB the messages need today, so the
messages get the same room whatever the catalogue weighs. With today's catalogue
that is a ~36K window, not exactly 32K. A test inflates the catalogue by 1.5 KB and
then 20 KB and replays all three scenarios
[src: file: backend/src/agents/runner_test.rs:3279#a_catalogue_change_does_not_flip_the_shortening_fixtures].

That decoupling drops the signal the old tests gave, badly, when the catalogue
outgrew the real 32K budget. A guard test keeps it, on its own: a 32K window must
leave at least 5,000 B for the messages beside the declared catalogue (9,450 B
today). When it fails, the catalogue outgrew the 32K budget and has to be trimmed,
or the default Ollama window raised
[src: file: backend/src/agents/runner_test.rs:3303#the_declared_catalogue_leaves_a_32k_window_enough_message_room].

**When the plain cut does happen, the result stays a valid envelope.** A JSON
result that nothing smarter could shorten used to be cut as text, handing the model
half a document. `truncated_json_envelope` now wraps its leading text instead:
`{"kronn_shortened": true, "truncated": true, "original_bytes": <size of the
response>, "excerpt": "<start of the original>", "note": "…"}`, with the largest
excerpt that keeps the envelope inside the budget once embedded in `messages`. An
envelope shortened again keeps the original size it reports. Content that is not
JSON keeps the plain cut and its note
[src: file: backend/src/agents/runner.rs:8019#truncated_json_envelope]
[src: file: backend/src/agents/runner_test.rs:3327#the_plain_text_cut_of_a_json_result_is_still_a_valid_truncation_envelope].

What this does not show: how often a model writes the extract it is shown, or
writes a good one unprompted. The "model" in that test reads the shape and follows
it; it is not a model. The broker's HTTP path refuses loopback addresses, so the
test stops at the extraction the broker applies (`apply_extract` on the parsed
spec) and does not go through a live `agent_api_call`.

## Judging a future request

### Workflow authoring (KT-673)

Discussion principals can call `workflow_list`, `workflow_get`,
`workflow_step_schema`, `workflow_create_draft` and `workflow_update`. Reads and
writes are scoped to general workflows plus the current discussion's project.
Create inherits that project unless an explicit null requests a general draft.
Update preserves the existing project and all omitted fields; supplied arrays
replace their stored array. Workers and workflow Agent steps cannot use this
authoring surface. [src: file: backend/src/api/agent_workflow_tools.rs:142-249#execute_workflow_tool]

Authoring always leaves the workflow disabled. Updating an enabled workflow
returns an explicit alternative: disable it in the Workflows UI or create a new
disabled draft. Neither enabling, triggering nor deletion is exposed here.
Cron and Tracker drafts default to concurrency 1. The existing workflow HTTP
handlers validate steps, references, allowlists and stored command bindings.
[src: file: backend/src/api/agent_workflow_tools.rs:80-137#draft_patch]
[src: file: backend/src/api/agent_workflow_tools.rs:228-230#enabled]
[src: file: backend/src/api/workflows.rs:2817-2877#validate_step_references]

The canonical schema lives in `workflow_step_schema.json`. MCP reads the complete
contract from `GET /api/workflows/step-schema`; native agents receive the same
contract and can request one step type or shared section. This avoids paying
the full schema on every turn while keeping both interfaces on the same source.
The examples are deserialized as real `WorkflowStep` values in regression tests.
[src: file: backend/src/api/workflow_step_schema.json:1-457#fields_by_type]
[src: file: backend/scripts/disc-introspection-mcp.py:9693-9695#call_workflow_step_schema]
[src: file: backend/src/api/agent_workflow_tests.rs:146-170#workflow_schema_sections_preserve_the_canonical_contract]

Ollama tool-result clamping preserves this authoring contract as complete JSON
before the next request is resized. The regression reproduces that production
order with the full native catalogue, an initial 32,768-token window (as in
production), and accumulated tool history. Ordinary large tool results still
undergo the existing clamping. This protects a protocol contract;
it does not establish model authoring quality. The measured attempts and their
execution checks are recorded in
[`native-workflow-2026-09-22.md`](../research/native-workflow-2026-09-22.md).
[src: file: backend/src/agents/runner.rs:7998-8015#workflow_step_schema]
[src: file: backend/src/agents/runner_test.rs:2414-2481#authoring_schemas_survive_clamping_when_tool_history_fills_the_window]

### Further capabilities

Ask which side of *execution* it falls on.

- Reading anything already inside the workspace, or one public URL: **in scope**, subject to
  the existing bounds.
- Producing or editing files in the workspace: **in scope** since 2026-08-18.
- Running an audit step: **in scope** since 0.14.2 (KT-924) for every HTTP agent,
  with the file tools scoped to the project and nothing beyond them — see
  [Audits](#audits-kt-924).
- Running a Quick Exec the human already saved: **in scope** since 0.13.0. It is execution,
  but of an argv Kronn owns, with no shell and inside the project that saved it.
- Generating an image or a video on a configured connection: **in scope** since 0.13.0, with
  the cost bounds above. This is the exception to "they may not execute anything" and it is
  narrow on purpose — Kronn owns the request, the room and the claim; the agent supplies a
  prompt and a modality it was told exists.
- Running a command, mutating git, reaching a private address, touching a path outside the
  workspace, or talking to an MCP server: **out of scope.** These are not
  missing features; granting one would mean building a second agent runtime beside the CLI
  path, with Kronn's privileges, for a model Kronn does not host.

A request that seems to need one of the excluded capabilities is usually a routing problem:
give the analysis to the HTTP agent and the execution to a CLI agent, in the same discussion.

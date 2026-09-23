# HTTP-agent capabilities — what API-mode agents may and may not do

> Scope: the agents Kronn drives over an HTTP chat API rather than a local CLI process —
> `Ollama`, `LiteLlm`, `Nvidia` [src: file: backend/src/agents/runner.rs:560].
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
any MCP environment is built [src: file: backend/src/agents/runner.rs:1010], and
`--mcp-config` is a CLI-only argument [src: file: backend/src/agents/runner.rs:1058-1061].
So "add a tool to the MCP server" never reaches an HTTP agent. Anything they must be able to
do has to exist as a **native tool in Kronn's own catalogue**.

That constraint is the whole reason a boundary has to be written down rather than inferred:
each capability is a piece of Kronn, executed with Kronn's privileges, on behalf of a model
that may be hosted by a third party.

## What they have

The workspace catalogue, eight tools [src: file: backend/src/api/agent_workspace_tools.rs:569]:

| Tool | Purpose |
|---|---|
| `web_fetch` | Fetch one http(s) URL server-side |
| `read_file` | Read one file inside the workspace |
| `write_file` | Create or overwrite one file inside the workspace |
| `list_files` | List a directory, optionally recursive |
| `find_files` | Glob search inside the workspace |
| `git_status`, `git_diff`, `git_log` | Read the workspace repository |

Plus Kronn's internal catalogue — plan and task tools, `qa_list`/`qa_run`, and `api_call`
against configured REST plugins [src: file: backend/src/api/agent_tools.rs].

And, since 0.13.0, media generation: `media_generate` and `media_job_status`
[src: file: backend/src/api/agent_tools.rs:420-445]. These sit in the orchestration
catalogue, so an HTTP agent replying in a discussion has them, and a bounded worker keeps
them — they are deliberately absent from the principal-only list a worker room strips
[src: file: backend/src/api/agent_tools.rs:661-685]. A Workflow Agent step does not: the
workflow branch returns before the orchestration tools are added.

Until then only CLI agents could generate an image or a video, because the tools were
declared in the MCP bridge and an HTTP agent structurally cannot reach it. The capability
existed and was announced to CLI agents by `kronn_intro`; the HTTP half of the fleet was
told about a feature it had no way to invoke.

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
  [src: file: docs/architecture/media-generation.md:74].
- An agent only learns a modality exists when a connection has a model configured for it —
  the worker catalogue lists modalities one per configured model and stays silent otherwise.
  An agent is never told it can produce a video that the request would then refuse.

Every one of these is bounded, and the bounds are part of the contract, not an
implementation detail:

- **Filesystem**: paths resolve inside the discussion's workspace and cannot leave it, by
  `..` or by symlink. The root is the discussion's `managed` workspace row when it has one,
  otherwise the path of the project the discussion belongs to; a workflow step has no
  discussion and resolves straight to its project's path
  [src: file: backend/src/api/agent_tools.rs:821-857]. The project fallback is the common
  case, not the exception: a workspace row is only created by orchestration, so on
  2026-08-19 just 16 of 395 discussions had one. Requiring such a row before granting the
  file tools was the original bug — the path was reachable all along.
- **Network**: `web_fetch` refuses private and loopback addresses before any request
  [src: file: backend/src/api/agent_workspace_tools.rs:109], times out at 20s
  [src: file: backend/src/api/agent_workspace_tools.rs:35], and caps the body at 256 KB
  [src: file: backend/src/api/agent_workspace_tools.rs:31].
- **Truncation is always announced.** A bounded read that returns part of a document sets a
  `truncated` flag, so a model can say it is reasoning from a partial view instead of
  concluding from one. Walks are capped at 20 000 entries and say so
  [src: file: backend/src/api/agent_workspace_tools.rs:48].
- **Git is read-only.** Status, diff and log; no commit, no checkout, no push.

## What they do not have, and why

- **No shell.** No arbitrary command execution, so no test runs, no build, no package
  manager. This is the load-bearing exclusion: a shell is a second runtime, and the CLI path
  already has one that works.
- **No mutating git.** No commit, checkout, branch, merge or push.
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
`[src: file: backend/src/api/agent_tools.rs:2634-2680]`. So an ordinary `@ollama` in a room
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
  [src: file: backend/src/api/discussions/ceilings.rs]

## Saved Quick Prompt generation settings (KT-670)

Single native `qp_run` launches preserve the saved agent, connection, tier,
model and bindings. Effort and output-token controls are captured in the same
transaction as the child discussion and its dispatch job. UI single-discussion
launches use the same capture helper. HTTP snapshots bind the discussion,
agent, connection, tier and resolved model; changing the target does not apply
the old target's controls. Existing discussions are not backfilled from a
mutable Quick Prompt. CLI effort keeps its existing snapshot format.
[src: file: backend/src/api/mcp_remote.rs]
[src: file: backend/src/db/discussion_launch_settings.rs]
[src: file: backend/src/db/sql/186_discussion_http_settings.sql]
[src: file: backend/src/api/discussions/crud.rs]

HTTP launches transmit explicit `reasoning_effort` and `max_tokens` on the
OpenAI-compatible wire. Ollama receives `think` and `options.num_predict` after
context fitting; an explicit thinking request removes Kronn's older Qwen
`/no_think` control message. Limits apply per provider request, including tool
round trips, rather than defining a total run budget. The provider may refuse
a setting its selected model does not support. Kronn does not retry by dropping
the saved control. [src: file: backend/src/agents/generation_settings.rs]
[src: file: backend/src/agents/runner.rs]
[src: url: https://docs.ollama.com/api/chat]
[src: url: https://docs.ollama.com/capabilities/thinking]
[src: url: https://docs.litellm.ai/docs/completion/input]

CLI transports currently have no implemented `max_tokens` mapping. A single QP
launch with that explicit limit is refused before a child or job is committed,
with an instruction to clear the setting or choose an HTTP provider. An explicit
effort on an unsupported transport is also refused. The runner applies the same
validation to explicit workflow Agent-step controls. Legacy batch/compare target
overrides are outside this single-launch snapshot change.
[src: file: backend/src/api/agent_quick_prompt_tests.rs]
[src: file: backend/src/agents/generation_settings.rs]
[src: file: backend/src/workflows/steps.rs]

The HTTP regression launches through the native QP tool and the discussion
stream, edits the template while the job is queued, then inspects both requests
received by a loopback provider. A separate Ollama fixture checks the actual
request after context fitting. These fixtures prove transport and persistence,
not live hosted-provider availability or model quality.
[src: file: backend/src/api/agent_quick_prompt_tests.rs]
[src: file: backend/src/agents/generation_settings.rs]

## Full catalogue by default; progressive loading is experimental (KT-685)

HTTP discussion agents receive the full native catalogue by default. Explicitly
setting `KRONN_TIERED_TOOLS=1` starts with the core tools and an index of the
`media`, `delegation`, `edit` and `automations` families. `tools_load` adds a family
for the remainder of the run. Bounded workers cannot expand their catalogue.
[src: file: backend/src/api/agent_tools.rs]

The native dispatcher now routes `tools_load` correctly. Earlier benchmarks used
a substitute dispatcher and their claimed savings did not validate this path.
A new campaign used the real executor, seven scenarios and all six installed
local models: full declarations succeeded in 35/42 cells, progressive loading in
17/42, with regressions for every model. This does not establish a saving at
comparable quality. The full protocol, individual costs, failures and limitations
are in [the campaign report](../research/native-tool-catalogue-2026-09-22.md).
[src: file: docs/research/native-tool-catalogue-2026-09-22.json]

Tiering reduces the initial declaration text, not the context capacity required
to reach every family. Window reservation counts the whole reachable catalogue,
including JSON separators, before loading a family. A regression test checks
exact byte counts and an unchanged window through all four family loads.
[src: file: backend/src/agents/runner.rs]
[src: file: backend/src/agents/runner_test.rs]

## Judging a future request

### Workflow authoring (KT-673)

Discussion principals can call `workflow_list`, `workflow_get`,
`workflow_step_schema`, `workflow_create_draft` and `workflow_update`. Reads and
writes are scoped to general workflows plus the current discussion's project.
Create inherits that project unless an explicit null requests a general draft.
Update preserves the existing project and all omitted fields; supplied arrays
replace their stored array. Workers and workflow Agent steps cannot use this
authoring surface. [src: file: backend/src/api/agent_workflow_tools.rs]

Authoring always leaves the workflow disabled. Updating an enabled workflow
returns an explicit alternative: disable it in the Workflows UI or create a new
disabled draft. Neither enabling, triggering nor deletion is exposed here.
Cron and Tracker drafts default to concurrency 1. The existing workflow HTTP
handlers validate steps, references, allowlists and stored command bindings.
[src: file: backend/src/api/agent_workflow_tools.rs]
[src: file: backend/src/api/workflows.rs]

The canonical schema lives in `workflow_step_schema.json`. MCP reads the complete
contract from `GET /api/workflows/step-schema`; native agents receive the same
contract and can request one step type or shared section. This avoids paying
the full schema on every turn while keeping both interfaces on the same source.
The examples are deserialized as real `WorkflowStep` values in regression tests.
[src: file: backend/src/api/workflow_step_schema.json]
[src: file: backend/scripts/disc-introspection-mcp.py]
[src: file: backend/src/api/agent_workflow_tests.rs]

Ollama tool-result clamping preserves this authoring contract as complete JSON
before the next request is resized. The regression reproduces that production
order with the full native catalogue, an initial 32,768-token window (as in
production), and accumulated tool history. Ordinary large tool results still
undergo the existing clamping. This protects a protocol contract;
it does not establish model authoring quality. The measured attempts and their
execution checks are recorded in
[`native-workflow-2026-09-22.md`](../research/native-workflow-2026-09-22.md).
[src: file: backend/src/agents/runner.rs:5379]
[src: file: backend/src/agents/runner_test.rs:1468]

### Further capabilities

Ask which side of *execution* it falls on.

- Reading anything already inside the workspace, or one public URL: **in scope**, subject to
  the existing bounds.
- Producing or editing files in the workspace: **in scope** since 2026-08-18.
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

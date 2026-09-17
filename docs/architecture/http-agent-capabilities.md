# HTTP-agent capabilities — what API-mode agents may and may not do

> Scope: the agents Kronn drives over an HTTP chat API rather than a local CLI process —
> `Ollama`, `LiteLlm`, `Nvidia` [src: file: backend/src/agents/runner.rs:565].
> This file is the boundary future capability requests are judged against (KT-338).

## Why a boundary exists at all

CLI agents run as a local process with the user's own login, a filesystem, a shell, and an
MCP bridge. HTTP agents have none of that: they are a request/response loop, and every
capability they appear to have is a **tool Kronn executes server-side on their behalf**.

The MCP bridge is deliberately not one of those capabilities. The HTTP path returns before
any MCP environment is built [src: file: backend/src/agents/runner.rs:1032], and
`--mcp-config` is a CLI-only argument [src: file: backend/src/agents/runner.rs:1080-1083].
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
[src: file: backend/src/api/agent_tools.rs:435-460]. These sit in the orchestration
catalogue, so an HTTP agent replying in a discussion has them, and a bounded worker keeps
them — they are deliberately absent from the principal-only list a worker room strips
[src: file: backend/src/api/agent_tools.rs:676-700]. A Workflow Agent step does not: the
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
  [src: file: backend/src/api/agent_tools.rs:836-872]. The project fallback is the common
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
`[src: file: backend/src/api/agent_tools.rs:2656-2702]`. So an ordinary `@ollama` in a room
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

## Judging a future request

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

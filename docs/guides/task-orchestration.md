# Delegate a planning task to an agent

Kronn 0.11.0 can run a planning task through a durable worker lifecycle instead
of leaving delegation as an informal chat instruction. One execution owns one
task, one worker, one child discussion and one isolated Git worktree. The parent
discussion remains the control room: it follows progress, reviews the delivery
and decides whether the candidate may reach the target branch.

The architecture and state-machine contract live in
[`adr-002-orchestration-multi-agent.md`](../design/adr-002-orchestration-multi-agent.md).
This page is the operator workflow.

## Before launching

1. Link the task to the parent discussion and give it a concrete Definition of
   Done. The worker receives the task description and this checklist as its
   brief.
2. Attach the discussion to a registered project repository. Kronn refuses to
   provision an unmanaged or unresolved path.
3. Name the integration branch (`base_rev`), or launch from a room that
   declares one. The child is pinned to that branch's commit; it never edits
   the parent checkout. Kronn never defaults to `main`; see
   [Where integration happens](#where-integration-happens). Integration
   requires the selected target branch's checked-out worktree to be clean, not
   every checkout of the project; a target no worktree has checked out has
   nothing to keep clean.
4. Confirm that the chosen agent is installed and authenticated. HTTP agents
   such as Ollama, LiteLLM and NVIDIA can also be selected when configured.

## Launch

Open the discussion plan, select an actionable task and choose **Run with an
agent**. The launch dialog makes the campaign policy explicit:

- agent, model and optional profile;
- project workspace and target branch;
- validation commands, one per line;
- maximum review rounds;
- allowed fallback agents;
- automatic continuation to the next ready task, off by default.

The 0.11.0 defaults are conservative: one active execution per campaign, one
CLI execution at a time, three review rounds, fast-forward-only integration and
preservation of failed or cancelled worktrees. A token budget is optional; no
value (`null`) means that Kronn enforces no campaign token ceiling, so operators
who require a hard cap must set one through the campaign API/MCP before launch.

Validation commands belong to the principal's launch policy. Kronn persists
them on both campaign runs and the implicit run created by a single-task
`task_exec_launch`; a worker manifest can report observations but cannot add,
remove or replace these gates.

### Where integration happens

An integration advances the target branch, and a checkout that holds that
branch is written to. So the target is never the owner's working checkout, and
never `main` by default:

- **The target is explicit or declared.** A launch takes the branch from
  `base_rev`. Without one it takes the integration branch its room declares: the
  `target_branch` of the room's active campaign (for example `integ/0.15.x`).
  With neither, the launch is refused with `integration_target_undeclared`; it
  does not fall back to `main`.
- **The checkout holding the target must be Kronn's.** A target checked out
  nowhere is advanced by a ref compare-and-swap that touches no checkout. One
  checked out in a Kronn-owned worktree is advanced inside it. A worktree is
  Kronn's when it sits under `<repo>/.kronn/worktrees`, under Kronn's autoCode
  root (`~/.kronn/autocode-worktrees`), under a folder listed in Settings >
  Workers (`managed_worktree_roots`, absolute paths) or in the project's
  workspace settings, or when Kronn's workspace registry records it (a managed
  discussion workspace or a workflow run). One checked out in the project's own
  working checkout is refused with `integration_target_owner_checkout`, even if
  the registry knows the path, and one checked out in any other worktree with
  `integration_target_foreign_worktree`. Those checkouts hold files the owner
  is working on, and fast-forwarding `main` there would also land a change
  without going through the pull request.
- **A managed worktree in use is not advanced.** If a run still uses the
  worktree that holds the target (an unfinished workflow run, an unfinished task
  execution, or a pending or running agent dispatch of its discussion), the
  integration is refused with `integration_target_busy_worktree`, naming the
  run, and nothing is changed. The execution parks like any refused
  integration; resume it once the run has finished.
- **The refusal comes at preflight and launch.** `task_exec_prepare` accepts the
  `base_rev` you will launch with (or checks the room's declared branch) and
  answers `launchable: false` with the same code; `task_exec_launch` refuses it
  before an execution, a child room or a worktree exists. It is not discovered
  at integration.
- **The status shows where it will land.** `task_exec_status` carries
  `target_branch`, `integration_worktree` (the worktree holding the branch,
  absent when none does) and `integration_target_state` (`kronn_worktree`,
  `not_checked_out`, `owner_checkout`, `foreign_worktree` or `unresolved`); the
  `full` view has them as `target_branch` and `integration_target`. A wrong
  target reads there before the integration, not at it.

To integrate somewhere the owner is not working, create the integration branch
and leave it checked out nowhere (or in a worktree under
`<repo>/.kronn/worktrees`), then declare it on the campaign or pass it as
`base_rev`.

### Default validations: fmt + clippy + targeted lib tests

In the CI gate model the PR is the gate, so the full test suite belongs to the
PR's CI, not to a local integration validation. A full local `cargo test`
averages about 20 minutes (5 runs, max 24.5 min, KT-1630 audit) against 3.3
minutes for `cargo test --lib`. The validations Kronn proposes are therefore:

- `cargo fmt --manifest-path backend/Cargo.toml --all -- --check`;
- `cargo clippy --manifest-path backend/Cargo.toml --all-targets -- -D warnings`;
- targeted lib tests, `cargo test --lib --manifest-path backend/Cargo.toml <filter>`.

The launch dialog states this under the validations field, and the worker brief
tells the worker the same (fmt + clippy + targeted lib tests; the full suite is
the PR's CI's). A validation that replays the full suite — bare `cargo test`,
`cargo nextest run` with no `--lib`/`--bin`/`--test`/`--doc` target and no name
filter, `make test-backend` or `make test-backend-cov` — is flagged as CI-owned:
the dialog shows a hint, and `task_exec_prepare` adds a non-blocking
`full_suite_ci_owned` entry to `warnings`. The flag never refuses: a principal
who keeps the gate still gets it run at integration. Both detectors are pure
string checks over the whitespace-split command:
`backend/src/api/orchestration.rs` (`is_full_suite_validation`) and
`frontend/src/lib/fullSuiteValidation.ts`. Both test suites read the one case
table `backend/tests/fixtures/full_suite_validation_cases.json`, so a case added
there is enforced on both sides and the detectors cannot drift.

Task orchestration is available by default in 0.11.0; there is no hidden feature
flag. Nothing starts automatically until an operator launches a task, and
`auto_continue` remains off unless the campaign explicitly enables it.

After launch, Kronn creates a child discussion and a managed worktree below its
managed worktree root. The execution card exposes the exact worker, model,
branch, worktree, elapsed time, tokens, cost and review state.

## Worker isolation

The managed worktree is only the worker's destination; Kronn also enforces the
execution boundary according to the worker transport:

- HTTP workers (Ollama, LiteLLM and NVIDIA) never receive a shell or a host
  filesystem path. They can inspect and mutate only through Kronn's bounded
  workspace tools (`search_text`, ranged reads, hash-guarded edits and mediated
  commits). This is the preferred path for cheap/local subtasks: a model cannot
  escape the worktree merely by emitting an absolute path or a destructive
  command.
- When the principal has already verified one exact file and inclusive range,
  it can launch a `prelocalized_edit` scope. Kronn then freezes the HTTP worker
  to one authoritative read and one CAS-bound `edit_lines`; a re-read, wider
  range or different path is refused before the tool executor. A successful
  edit exposes only commit, then delivery. The worker reports semantic evidence
  in Definition-of-Done order; Kronn injects opaque DoD ids, task identity, HEAD
  and committed file inventory from trusted state. This is the preferred
  contract for truly atomic local-model edits, not for discovery or cross-file
  reasoning.
- For a pure insertion, the principal uses
  `prelocalized_insert_after` with one verified `anchor_line` instead. The
  worker supplies only the new text; Kronn preserves the anchor bytes
  mechanically and refuses a stale receipt or a different target. Prefer this
  over asking a small model to reproduce an existing paragraph inside an
  `edit_lines` replacement.
- Claude Code workers run with Claude's OS-level sandbox enabled and required.
  The unsandboxed escape hatch and global permission bypass are disabled, and
  user/project settings are ignored so they cannot add writable directories.
  Normal discussions keep their existing permission policy.
- Spawned host CLI workers keep their native shell/editing capability, but use
  the same projected delivery contract as HTTP workers: they author semantic
  evidence only, while Kronn injects the task/Git/DoD mechanics. Joined CLI
  sessions remain on the full public DeliveryManifest contract.
- Codex workers explicitly use `workspace-write` while ignoring user config and
  exec-policy files that could widen writable roots. A discussion-level
  full-access preference never upgrades a spawned worker to
  `danger-full-access`.
- Other CLI adapters never inherit their global `yolo`, `trust all` or
  `allow-all-tools` switches in task-worker mode. If their non-interactive CLI
  cannot proceed without such a bypass, the task fails visibly and can be
  reassigned; Kronn does not trade confinement for apparent progress.

Every git command Kronn runs on a worker's worktree (inspect, stage, commit,
merge, abort, doc re-anchoring) runs in Kronn's process, not the worker's
sandbox. It is therefore built by one function that pins the repository to the
registered project checkout (never to the worktree's own `.git` file, which the
worker could redirect), gives git no hook, file-system monitor or signing
program (so no program a worker names can run), and refuses with `WorktreeTampered` when the pointer was
changed; the worker cannot write that file in the first place. The project's
`worker_commit_hooks` setting (`off` by default, or `sandboxed`; a hook never runs
as the host user)
decides which gating hook Kronn itself runs, and a refusing hook is reported as
`HookRefused` with its output, not as a conflict; `commit-msg` checks other than
the DCO sign-off Kronn adds are not run. See
[worker-commit-hooks](../operations/worker-commit-hooks.md).

Sandbox startup failure is a launch failure, not a reason to run unsandboxed.
Likewise, delivery is accepted only through the execution-scoped capability and
against the exact managed branch/HEAD.

## Follow and communicate

Open the child discussion from the execution card to inspect the worker's live
messages and tool traces. Continue coordinating in the parent discussion:
Kronn routes worker delivery and attention messages back to the parent without
requiring every agent to sit in a blocking room wait.

The execution is durable across browser reloads and backend restarts. A restart
reconciles persisted execution, dispatch and worktree state; it does not infer
success from a stale `Running` row. When Kronn cannot prove the safe next step,
the execution becomes interrupted, blocked or escalated with a reason instead
of silently continuing.

A child assigned to an exact joined CLI is CLI-owned even when that session
is idle, has left, or has not reconnected after a restart. Its durable execution
assignment disables the native responder: an untargeted worker message cannot
silently launch a second agent into the same worktree. Legacy queued native
jobs are retired without another provider attempt, and ownership is checked
again immediately before provider start. The room toggle cannot override this
assignment. Reassign the execution explicitly to a native worker to release
CLI-only containment; this cancels stale pending jobs before enabling the new
worker. Ordinary rooms retain their existing native-agent toggle.

### Quiet work and useful milestones

Technical progress stays in the execution's child discussion; it is not
automatically copied into the parent. Factual milestones are distinct: a joined
CLI can explicitly publish one to the real parent, while a native/HTTP worker
without that tool asks the principal to relay it. This follows the accepted
KT-635 brief policy, not a guarantee that every external host CLI obeys it.
An unreviewed delivery never appears as an accepted report. After approval,
Kronn derives one fixed-layout report; replay does not produce another.
[src: file: backend/src/api/orchestration.rs:12160#has_room_publication]
[src: file: backend/src/api/orchestration.rs:12197-12221#parent_milestones]
[src: file: backend/src/api/orchestration.rs:10115-10139#publish_accepted_delivery_or_warn]
[src: file: backend/src/api/orchestration_tests_up.rs:22478-22588#approving_a_delivery_publishes_one_derived_report_in_the_parent]

Native conversational handoffs are a separate opt-in from launching a planning
task. Their paid/unknown allowance is configurable and shared by the originating
human turn. Removing that allowance explicitly does not remove structural
guards: only attached, not-already-scheduled targets can run, delegation depth
remains bounded and local handoffs retain a fixed ceiling. A typed wake to a
joined CLI never silently substitutes a native responder.
[src: file: backend/src/models/setup.rs:1070-1086#agent_handoff_paid_limit]
[src: file: backend/src/db/discussions.rs:1703-1704#MAX_AGENT_HANDOFF_DEPTH]
[src: file: backend/src/db/discussions.rs:1711-1746#agent_handoff_root]
[src: file: backend/src/db/discussions.rs:1921-1961#cli_mention_targets]
[src: file: backend/src/db/discussions.rs:2020-2063#already_scheduled]

### Warming, queueing and which clock applies

A launch goes through several waits before the worker's first tool call. Only
the last one is the worker's own; the others are Kronn's, so the silent-worker
clock (KT-1247) does not run during them (KT-1689).

| Phase | Execution status | What shows in the status | Clock |
| --- | --- | --- | --- |
| Worktree creation, lane provisioning, seed clone, lane seeds | `Provisioning` | `phase: warming`, the `warming_steps` | None: the activity clock only expires a `Working` execution. |
| Build-slot wait (`gate_wait`, up to the first-build timeout, 60 min by default) and lane first build (`first_build`, up to the first-build timeout) | `Provisioning` | `warming_steps` with their durations | None, same reason. A long wait or a 24-min build never counts as inactivity. |
| Toolchain smoke check, brief, dispatch | `Provisioning` → `Working` | `smoke_check` step | None until the dispatch exists. |
| Queued for a worker slot (task-worker cap, agent or host caps) | `Working`, dispatch `Pending` | `phase: queued`, reason `task_worker_limit:<running>/<cap>`, queue position | **Queue deadline** only (below). The activity clock is suspended while the dispatch is `Pending`. |
| Worker's turn: claimed, agent started, tool calls, messages, commits | `Working`, dispatch `Running` | `launching`, `tool_activity`, `upstream_wait` | **Activity clock**: 20 min without a tool call, message, commit or delivery interrupts the execution (`activity_timeout`, resumable). It starts at the claim of the dispatch, not at its creation, and a single tool call in flight counts as activity up to its cap. |

The activity clock default is 20 minutes
(`KRONN_TASK_ACTIVITY_TIMEOUT_SECS`, or the campaign's resilience policy, which
wins). A tool call in flight (a cold build run by the worker itself) is activity
up to `KRONN_TASK_TOOL_IN_FLIGHT_CAP_SECS` (2 h by default).
[src: file: backend/src/db/orchestration.rs:8164-8169#DEFAULT_ACTIVITY_TIMEOUT_SECS]

**Queue deadline.** An execution queued longer than the queue deadline is
reported, never interrupted: the principal room receives one orchestration
event per dispatch naming the reason (`task_worker_limit:2/2`) and the queue
position, with the options (keep waiting, free a slot, take over). The execution
stays `Working` and its dispatch stays queued; it starts when a slot frees. The
deadline is 6 hours by default and is set host-wide with
`KRONN_TASK_QUEUE_TIMEOUT_SECS` (a positive number of seconds; an unusable value
falls back to the default). It counts from the creation of the queued dispatch.
A resumed execution queues a new dispatch and gets its own deadline.
[src: file: backend/src/db/orchestration.rs:8192-8195#DEFAULT_QUEUE_TIMEOUT_SECS]
[src: file: backend/src/db/orchestration.rs:8203-8205#queue_wait_reported]

### Highlight a message without changing the task

Choose **Important message** above the ordinary composer, enter text and select
Information, Decision or Blocker. A task link is optional and selects an
existing task from this discussion's plan; it does not create, edit or complete
that task. Tasks are managed in the plan, not through this publication form.

The publication key is requested only inside the open form; use the existing
credential enrolled in Settings. Your normal draft stays independent. Closing
the form or visiting Settings preserves its text in memory, but reloading the
browser does not. If an outcome is uncertain, retry reconciles that exact
attempt before another write. See [important messages](../architecture/important-messages.md#human-form-0130-kt-643)
for the lifecycle and [publication authority](../architecture/important-message-publication-authority.md)
for enrollment, recovery and security limits.
[src: file: frontend/src/components/ImportantMessageForm.tsx:87-110#onGrantChange]
[src: file: frontend/src/lib/importantMessageDraft.ts:16-20#drafts]
[src: file: frontend/src/lib/submitImportantMessage.ts:23-35#submitImportantMessage]

## Review and integrate

When the worker delivers, the execution enters **Awaiting review**. Inspect the
delivery manifest and diff, then choose one of:

- **Approve** — record a result and non-empty evidence for every DoD item against
  the delivered attempt and exact HEAD. Kronn then builds and validates that
  candidate in an ephemeral integration worktree. The selected target branch
  advances only by a guarded fast-forward after checking the expected target
  SHA and clean worktree, with a backup ref. For a target checked out in a
  worktree, these checks followed by `git merge --ff-only` are not an atomic
  compare-and-swap transaction; a target no worktree has checked out advances
  by a `git update-ref` compare-and-swap instead.
  [src: file: backend/src/core/worktree.rs:2796-2824#write_backup_ref]
  [src: file: backend/src/core/worktree.rs:3594-3614#fast_forward_target_to]
  [src: file: backend/src/core/worktree.rs:3539-3592#update_ref_target_to]
- **Request changes** — enter concrete feedback. The same worker, discussion,
  branch and worktree resume for the next bounded review round.

Review-budget exhaustion escalates to the parent; it never converts a rejected
candidate into an approval.

Normal integration and restart recovery resolve the shape of that pinned local
target branch: checked out in exactly one worktree, or in none. Several
checkouts, a target that is not a local branch, a changed target tip, or dirty
files in the one checkout stop the operation without switching or overwriting
another checkout. Unrelated changes in the registered project's
main checkout do not block a clean target in another worktree. See
[`pinned-target-worktree.md`](../testing/pinned-target-worktree.md).
[src: file: backend/src/core/worktree.rs:3215-3222#integration_target]

Worker-written DoD claims, a checked box in the live Planning task and a
principal approval without attempt-scoped evidence are all insufficient. This
keeps local workers useful without asking the model that produced the change to
be its own quality gate.

### Who settles a doc-citation conflict

Two lanes that touch the same cited file both re-anchor the same
citation lines of the same doc, and the integration merge
of the target into the candidate then conflicts on text that differs only by line
numbers. That conflict is Kronn's to settle, not the worker's and not the
principal's (KT-1669). When **every** conflict hunk is in a doc the citation gate
reads (`docs/` and `backend/docs/`, release notes aside) and its two sides differ
only inside those markers, the integration, without a worker turn, takes the
target's side for those hunks.

The re-anchoring itself does not wait for a conflict. On **every** candidate,
conflicted or not, the integration:

1. re-anchors the citations of every doc onto the merged files, the same
   computation `make docs-reanchor BASE=merge` runs mid-merge, and stages the
   result into the merge commit. A branch that already descends from the target
   (nothing to merge) gets the same re-anchoring in a commit of its own;
2. compares the citations that do not resolve in the candidate with those that
   did not resolve on the target, doc by doc, and requires that no citation that
   resolved on the target is unresolved now. A citation already broken on the
   target never blocks a merge;
3. records it. The merge commit body counts what was settled, the journal keeps
   how many anchors moved, and the accepted delivery report carries an
   `integration:` attention point naming the docs, so a reviewer sees the worker
   was not asked.

Everything else still goes back to the worker, as for any merge conflict: a
conflict on prose or code, even beside citation-only hunks, and a citation the
candidate leaves unresolved that resolved on the target (its symbol is gone, it
moved too far to be guessed, two occurrences are equally near, or the doc to
rewrite has unstaged changes). That bounce undoes the merge (or the
re-anchoring commit) and names the exact citations (`doc:line [src: …] reason`)
in the worker's message and in the principal's notice.

A lane therefore keeps only the citations of the docs it edits resolving, which
is its own gate, and never rewrites the target's anchors to dodge a conflict: the
integration moves every anchor its code shift only displaced, in any doc, so the
integration branch stays green on the strict `docs_citations_resolve_in_the_repo`
test without a release-time sweep. A citation the lane breaks for good (symbol
removed or renamed) is the lane's to fix; the worker brief says so. The merge a
worker finishes with `task_exec_commit` runs the same settlement.
[src: file: backend/src/core/worktree.rs:2839-2943#build_candidate_detailed]
[src: file: backend/src/core/worktree.rs:3063-3126#settle_branch_docs]
[src: file: backend/src/core/docs_citations/merge.rs:318-334#resolve_anchor_only_conflicts]
[src: file: backend/src/core/docs_citations/merge.rs:747-807#newly_unresolved]
[src: file: backend/src/api/delivery_publication.rs:91-123#citations_settled_point]

### The worker's own commit settles the citations its code shifted

A worker that adds lines to a file many docs cite (`backend/src/api/orchestration.rs`
is the usual one) moves anchors in docs it never opened, and it does not run the
strict gate. Waiting for the integration to bounce those citations cost a whole
review round (KT-1721: 79 and 105 unresolved on two lanes). So every plain
`task_exec_commit` (and `git_commit` for an HTTP agent) is followed, in the same
call, by the repo-wide re-anchoring against the execution's pinned base:

1. every citation of every doc whose cited file the branch changed since the base,
   not only the docs in the commit, is re-pointed under the integration's rules (a
   `#symbol` the file defines once is placed whatever the distance, also when the
   cited range now lies past the end of a file that shrank (KT-1684); the history
   of the base, the previous commit and the branch settles the rest);
2. the re-pointed docs are committed on their own (`Re-anchor doc citations moved
   by the worker's code`) and listed under `doc_citations.reanchored`;
3. a citation the evidence cannot place, or whose symbol was removed, stays as it
   is and is listed under `doc_citations.unresolved` (with `unresolved_count`),
   limited to what resolved on the base and no longer does. The worker points
   those by hand, commits the doc, and only then delivers.

A doc or a cited file with unstaged changes defers the re-anchoring
(`doc_citations.reanchor_deferred`) rather than sweeping work in progress; the
next commit re-anchors. A commit that finishes a merge keeps the merge's own
settlement above.
[src: file: backend/src/api/agent_workspace_tools.rs:762-789#git_commit_payload_reanchoring_docs]
[src: file: backend/src/core/docs_citations/merge.rs:650-662#reanchor_branch_leaving_unplaced]

### Who a bridge token can be

The `task_exec_*` tools name their caller in the request body: a joined
session, or the room's own agent during its turn. Both are facts any reader of
the room sees, so for an agent Kronn launched the body is only a claim and the
launch's own bridge token decides the role (KT-1962):

- **Principal** (prepare, launch, status, resume, cancel, reassign, review):
  the grant runs in the execution's parent room, does not own the worker's
  room and was not minted for that execution. The room-agent identity also
  wants the grant of that room's agent, launched with the provider the turn
  names (a grant that names no launched agent is refused); a workflow step or another launch hosted in the room is a principal
  by its own joined session only. Any other grant gets the answer of an
  unknown execution, and a review it sends pins no principal session.
- **Principal beside its worker**: when the worker is a CLI session of the
  principal's own room (it accepted an offer and works from there), a grant of
  that room may be the worker's. The session a token names as the principal's
  must then be one that very launch joined or resumed through the bridge
  (`peer-join`, `peer-resume`, the workflow step join); a co-member that only
  read the session id is refused, and the worker's own session never makes it
  the principal. For the same reason a token joins a room (`peer-join`) as a session
  already active somewhere only when its own launch holds it: an invite and a
  session id read in a room would otherwise hand out that session's resume
  credential, or end it where it is and make the caller its holder. The same
  holds for a room joined through a contact. And the launch that holds the
  worker's session is the worker, whatever other session it joined and names.
  Known limit: a new launch in that room that holds neither session is a room
  member like any other, so a principal by design. A session held
  elsewhere answers « not yours to take »; its bridge resumes it by itself. A
  caller without a token (a host CLI) joins as before.
- **Worker**: a spawned worker (deliver, commit, status) is the grant minted
  for that execution and no other, even one that runs in the worker's room. A
  worker that names its joined session (deliver, review) holds a grant minted
  for that execution, or one that runs in the room this session is joined to.
- **Offer**: a grant accepts an offer only for a session that runs in one of
  its own rooms.

A host CLI and the web UI carry no such token and are judged on the declared
identity alone, as before. A principal whose token adopted the child room is
refused like a worker: it acts through a token that owns only the parent room,
or through the human UI.

### A DoD item the principal checks live, after integration

Some items cannot be proven before the code is integrated and running (a replay
on a real input after a restart). Such an item is `verification:
"post_integration"` — set on the task's `definition_of_done` with `task_update`
while the execution is live, or, at review, with a `dod_verifications` entry
`{dod_id, met: false, evidence, verification: "post_integration"}`. Only the
principal of the task's execution, or a human, may set or remove it, and tick
it: a worker's own session, an HTTP agent and an anonymous agent are refused.
`task_create` cannot declare one for an agent, since a new task has no execution.

- The worker still lists the item in its manifest, honestly `met: false`; the
  manifest validation and the approve guard accept it unmet and need no
  principal evidence for it. Every other DoD item keeps the rules above.
- Approve integrates normally. The execution ends `Done`, but the plan task
  stays `in_progress` with the item listed as `live_checks` on its summary.
  (Integration into a campaign or feature branch keeps the task open anyway:
  see « Integrated into a feature branch » below.)
- The principal runs the check, then ticks it with `task_update_dod`
  (`completed: true` and non-empty `evidence`). A bare tick is refused, as is
  ticking it through a `definition_of_done` replace, and downgrading it to a
  worker check in the same write that ticks it. The task closes on the last pending live check of an integrated execution.
  A human ticks it in Kronn (plan panel or Planning page), which asks for the
  proof and sends it as `evidence`. The same actors may tick once the execution
  is over; an agent bound to another room is refused and asks the human.
  The bridge proves the principal with the identity the `task_exec_*` tools
  send, derived by the bridge and never named by the model (KT-1954): the
  session joined to the execution's parent room, or the room's own agent during
  its turn. A room agent's identity is made of facts any reader of the room
  sees, so with a bridge token the grant itself must be the principal's: it
  is the parent room's agent (a workflow step or another launch hosted there
  is not) and does not own the worker's room. A session id is also a fact
  the journal shows: on any planning write a bridge token's actor session is
  marked as a claim that authorises nothing, and the joined-session proof holds
  only for a grant that runs in the parent room and not in the worker's (a
  workflow step that joined that room is a principal by its own session). The
  worker's session, a session of the worker's room, another task's worker and a
  bridge with no identity stay refused. The proof is
  asked for a live check only, and the journal names the identity that was
  proven. Before 0.15.3 the tick carried no joined
  session, so every agent principal was refused its own check.
- Unticking a live check (by `task_update_dod` or an array replace, principal or
  human only) clears its evidence, and a `done` task goes back to `in_progress`:
  the proof is owed again and the stale one cannot re-tick it.
- A discussion export carries each item's `verification` and `evidence`; the
  import restores them as they were, without a principal session (its own
  trusted door, never reachable from HTTP or MCP). A ticked live check whose
  evidence is missing or unusable comes back pending, with a warning.

### What the accepted report says, and whose word it is

The published report keeps two records apart, because they answer different
questions.

**Validations** are the worker's own: the commands it ran, and what it honestly
could not run. A worker with no browser in its worktree reports its Chromium
checks as `skipped`, and that stays `skipped` in the report for good. Approval
does not turn it into a `pass`; the worker never claimed one.

**Verified by the principal at review** is the evidence the reviewer produced
itself, from `dod_verifications` on the approving decision, addressed to the DoD
items by their opaque ids. It appears only when the review supplied some. A
review that added no evidence of its own produces no such section — an empty
heading would read as "the principal checked nothing", which is a claim rather
than an absence.

Every verification attached to an approved delivery reads `met`: approval
requires it, and a decision carrying any `met: false` is refused before it can
accept the delivery. The renderer preserves its supplied data rather than
inventing a verdict.

The limits worth knowing:

- The report is derived once, from the review that accepted the attempt. A later
  validation is not folded back into it; it belongs to the room, or to a new
  attempt.
- A decision that cannot be parsed leaves the report without principal evidence
  rather than without a report. Publication is total once a delivery is
  accepted, so a broken decision degrades the report instead of suppressing it.
- Nothing in this section is inferred. No metric, timestamp or verdict is
  derived from the worker's own claims.

### A DoD item carried to another ticket

An item that cannot be proven within this task (it needs lots that belong to
another ticket) is carried, not moved by hand. The principal approves with a
`dod_verifications` entry `{dod_id, met: false, evidence, carried_to: "KT-1290"}`.
The carry is accepted only when the carrier is another task that exists, is
not `done` or `archived`, shares a project with this task, and holds the
requirement as an unticked DoD item with the same sentence (case and spacing
aside). The sentence compared is the one the worker was briefed with: the
execution records the `{id, sentence}` of its DoD items in its `created`
event, and a `task_update` that removes an item records it (id and sentence)
in the `removed_dod` of its `updated` event. The approve then integrates
normally (validations, evidence, auto-tick of the other items); the item is
detached from this task and a `dod_carried` event on the task names the
carrier, the item's sentence and the principal's evidence. The detach runs in
the review checkpoint's transaction, so a refused or failed approve detaches
nothing. A carry that claims `met: true`, names itself, a missing or a closed
task, a task of another project, or a task that does not hold the requirement
is refused with that cause.

The delivered manifest is judged against the execution's launch-time DoD
snapshot, not against the task's current items: an id the principal removed or
moved since the delivery is a known id, never "unknown". An item the task no
longer holds that the manifest reports unmet still needs the explicit carry
above; omitting it refuses the approve with that id.

When no record names the sentence (an execution launched before the snapshot
kept sentences, whose item was removed without a trace), the carry is refused
instead of matching any open task: state the requirement on the carrier and
restore the item on this task before carrying it.

### The Definition of Done after an integration

An approve that integrates ticks the DoD items it verified, in the same
transaction that marks the task `done` (integration into the default branch) or
integrated and awaiting its merge (any other branch, below). An item the principal named in
`dod_verifications` carries the principal's evidence; one it did not name (a
human approve supplies none) carries the worker's own `dod_status` evidence.
The evidence is stored on the item (`evidence` on the DoD item) and shown with
it. Only worker items are ticked this way: a `post_integration` live check
(above) is never ticked by an integration, whatever the review said about it,
because its evidence must come from the live check itself.

A task does not reach `done` with an unticked worker DoD item. The one
exception is the KT-1254 live check, which is not asked of the approve and keeps
the task `in_progress` until the principal ticks it with evidence. There is a
single model for it: the item's `verification` column, no sentence convention.

The guard runs twice. BEFORE the integration touches the target, an item that
neither the approve nor the delivery verified (one added between approve and
integration, say) holds the approval as `IntegrationRefused`: the target does not
move and no execution is left parked with its code already on it. The
`Integrated` checkpoint then re-checks in the same transaction as the `done`
move, and rolls back on a leftover.

What a later manual write does to the evidence of a worker item:

- `task_update_dod` with `completed: false` (or a definition-of-done replace that
  unticks the item) drops it: the proof went with the tick.
- Ticking again without `evidence` leaves a ticked item's evidence alone, and
  ticks an unticked one with no evidence (a manual tick invents none).
- Ticking with `evidence` replaces it; a replace that keeps the item ticked, in
  the same mode, keeps it, and changing the mode drops it.

Only the integration path enforces the guard; a task a human sets to `done` in
the Planning page is not checked.

### Integrated into a feature branch

`done` means the change reached the default branch. An integration into any
other target (a campaign branch such as `autocode/kt-1495`, a review branch)
ends the execution `Done` but leaves the plan task `in_progress`, **integrated
and awaiting the merge**: the summary carries `awaiting_merge` (the branch and
the integrated commit), and the planning page, the board and the discussion plan
show « Intégrée dans <branche> — en attente de fusion » instead of `done`; the
Planning list and the project Tasks panel carry it too (`list_tasks` builds each
row with the same summary), and the plan counters (`stats`, and the discussion
monitor's) count it in an `awaiting_merge` bucket apart from `in_progress` and
`done`. The
branch name is shortened in the pill and complete on its title. A repository
with no discoverable default branch has no other branch to tell apart from it:
there the integration closes the task.

An awaiting-merge task is **satisfied** as a blocker: the next task of the
campaign builds on that branch, so it launches (preflight, launch boundary and
the start checkpoint), the plan and the monitor counters stop counting it
blocked, and the campaign completes once every active task is done or awaiting
its merge. One rule serves them all: `PlanningTaskSummary::satisfies_dependents`.

The task closes (`done`, `via: orchestration_integration`) when any of these
holds, checked at boot, after every integration into the default branch and
every five minutes:

- the integrated commit is an ancestor of the default branch, local or the
  fetched `origin/<default>` (after a GitHub merge only the latter moves until a
  pull);
- GitHub reports the task's own work item done: the issue its `github-N` tag
  names is closed (not as « not planned »), or that number is a merged pull
  request; or a pull request the task links (`…/pull/N` of the project's
  repository) was merged **from the branch the task was integrated into**. A
  link to an earlier issue or to a pull request from another branch is a
  reference and never closes the task. A squash merge, how this repository
  merges, never makes the integrated commit an ancestor, so this is its only
  signal. It needs the machine's GitHub token; without it, or when GitHub does
  not answer, the task stays awaiting;
- a human closes it from the planning page.

The sweep reads Git with `git merge-base --is-ancestor` off the async executor.
The periodic sweep (and the one right after boot) first fetches the default
branch from `origin` (30 s at most, no prompt, best effort), because a merge
made on the remote moves nothing in the host checkout otherwise; the sweep that
follows an integration into the default branch does not, its own ref has just
moved. A task that owes a `post_integration` live check when its branch is
merged leaves the wait once (one `integrated_pending_live_check` event) and
stays `in_progress` on the check alone; a live check ticked before the merge
lets the merge close the task.

## Stop or reassign

**Stop** cancels live dispatch and always keeps the child branch with its
commits. What happens to the child worktree is the campaign's
`cancellation_cleanup_policy`, which defaults to `remove_if_clean` [src: file: backend/src/api/orchestration.rs:9554]:
a clean worktree is removed (its provenance is kept), while one with
uncommitted work, or whose removal Git refuses, is preserved
[src: file: backend/src/api/orchestration.rs:11000]. Set the policy to
`preserve` to keep the worktree whatever its state. Stop does not force-delete
local work. It never changes the task's
status (KT-1267): an `in_progress` or `done` task stays so, because the work may
already be integrated by hand and the principal decides. To run the task again,
set it back to `todo` first (`task_update status=todo`); the launch preflight
refuses any other status.

**Reassign** is available only in resumable worker states. Choose another
allowed agent/model; Kronn keeps the task, child discussion and Git lineage so
the replacement can continue from the same evidence. Reassignment is the normal
response to provider quota/rate limits. Provider, tier, explicit model and
profile change together; Kronn refuses the reassignment if it cannot update the
durable child discussion that the runtime will actually execute.

**Automatic fallback of a task worker (KT-1713).** When the worker's provider
refuses a turn for a quota, a rate limit or a missing login and its agent has a
fallback configured, Kronn reassigns the execution to that fallback in the same
child room and worktree, and the parent room says « Task worker execution
<id>: … → continued with … ». Which refusal follows which setting:

- A **quota** refusal first looks at the launch's own opt-in,
  `quota_fallback_agents` (or the campaign's allowed agents), then at the
  agent's chain.
- A **rate limit**, an **outage** or an **authentication failure** follows the
  agent's chain (Settings > Agents) at once, even when the launch named no
  `quota_fallback_agents`.

Three rules:

- A generic worker (no `worker_scope`) never moves to an HTTP model (LiteLLM,
  NVIDIA, Ollama), whether the chain offers it (KT-1991), the launch names it in
  `quota_fallback_agents` or the campaign merely allows it (KT-2000). Such a
  model has no shell, so it can neither build, run the tests nor commit. It is
  skipped and the next agent is tried: a mixed list gives its CLI agent. Only a
  prelocalised (`scoped`) worker continues on an HTTP model.
  [src: file: backend/src/api/discussions/runtime.rs:932#needs_shell]
  The rule covers the moves Kronn makes on its own, and the reassignment
  itself refuses such a quota fallback, whichever step chose it. An explicit
  choice of an HTTP agent for a generic worker, by a person or by the
  principal (at launch, with `task_exec_reassign` or in the UI), stays
  possible.
  [src: file: backend/src/api/orchestration.rs:15779#QuotaFallback]
  When nothing is left, the worker is not moved, and what follows depends on
  the path:
  - a **failed turn** (rate limit, outage, authentication): both rooms read the
    chain's stop line, « Claude Code: rate limited → no fallback (No fallback
    could take over from Claude Code: LiteLLM cannot take this turn: it has no
    shell to build, run the tests or commit.) », then the dispatch fails and
    the execution is `Interrupted`;
  - a **quota refusal**: the same stop line when the agent has a chain, then the
    execution is `Escalated` and the parent room reads « Intervention humaine
    requise — quota épuisé … ». An HTTP agent skipped in
    `quota_fallback_agents` is not named there;
  - a **launch refused, or retries that ran out** (on a worker a fallback had
    moved): the execution is `Interrupted` and both rooms read « Worker not
    started — … », the launch's reason, the chain's own reason (« Fallback
    chain: No fallback could take over from … it has no shell … ») and « No
    usable fallback link is left ».
- Codex is never chosen for it by the agent's chain: its sandbox cannot write
  the linked worktree's git directory, so it could not merge the target, commit
  or deliver. The room names this reason when no other fallback can take over,
  and you reassign yourself. `quota_fallback_agents` and the campaign
  allow-list are an explicit opt-in: a quota refusal does move a worker to
  Codex when the launch or the campaign lists it.
- The move lasts as long as the quota window. Once it has ended, resuming the
  execution (or the periodic quota sweep) hands it back to the worker you
  declared, and the room says « … back on its declared worker … » instead of
  redispatching the fallback. Reassigning by hand ends this memory.

See [agent-fallback-chain](../operations/agent-fallback-chain.md#task-workers-kt-1713)
for the mechanism.

## Resolve common failures

| Symptom | Safe response |
| --- | --- |
| Agent quota or authentication failure | Reassign to another allowed agent, or retry after quota renewal. |
| Validation failed | Open the child discussion, fix the reported command, deliver again. |
| Target branch moved | Refresh/relaunch against the new target; Kronn will not overwrite it. |
| Dirty target worktree | Intentionally preserve or commit changes in the selected target branch's checkout, then retry integration. Do not move unrelated files from another checkout. |
| Target checked out in several worktrees, or not a local branch | Leave at most one checkout of the pinned local target branch, and make sure the target is a branch, not a tag or SHA, before retrying; Kronn does not choose or switch a branch for you. |
| `integration_target_undeclared`, `integration_target_owner_checkout` or `integration_target_foreign_worktree` at preflight or launch | Name an integration branch (`base_rev`) or declare one on the room's campaign, and pick one that is checked out nowhere or in a Kronn-owned worktree. Kronn never integrates into the owner's checkout or defaults to `main`. |
| `integration_target_busy_worktree` at integration | The named run still uses the worktree that holds the target. Let it finish (or cancel it), then resume the integration; nothing was moved. |
| `integration_target_foreign_worktree` for a worktree Kronn itself created elsewhere | Add its parent folder to Settings > Workers > Managed worktree folders (or to the project's workspace settings). |
| Missing workspace | Restore/register the project workspace; do not edit execution rows manually. |
| Interrupted after restart | Read the persisted reason, verify the worker/worktree, then use the guarded resume/reassign action. |
| Initial CLI offer never accepted | Resume/reassign do not apply to `Blocked` from `Provisioning`. The principal may cancel with workspace preservation, verify the old execution is terminal, set the task back to `todo` (`task_update status=todo`; the cancel keeps its status, usually `in_progress`, and the launch preflight refuses any other status), then select the exact available CLI and prepare a new execution for the same task. Launch only after a fresh launchable preflight confirms no active duplicate. Do not silently substitute a different CLI identity. |
| Review budget exhausted | Decide in the parent discussion whether to expand scope, create a follow-up task or stop. |

Do not repair orchestration by manually changing SQLite rows. The status,
dispatch, discussion and Git lineage form one audited state machine; a manual DB
change can make an unsafe transition look valid.

## Back up and roll back a 0.10.0 installation

Before first starting 0.11.0 against a persistent database, make a consistent
SQLite backup. Kronn automatically writes `<database>.db.backup` when pending
migrations exist, after checkpointing the WAL. For an operator-controlled copy,
use the Settings backup action or SQLite's online `.backup` command documented
in [`db-backup.md`](../operations/db-backup.md).

Database migrations are forward-only. Rolling the application binary back to
0.10.0 does not roll the database schema back. If application rollback is
required, stop Kronn and restore the pre-upgrade database **and** its matching
configuration backup before starting 0.10.0. Keep the failed upgraded database
for diagnosis.

Migration `127_task_orchestration` is frozen: it has run on persistent 0.11.0
databases and must never be edited in place. Every later orchestration schema
change uses migration 128 or above. The migration runner records names in
`_migrations`; an edited migration 127 would not replay and `IF NOT EXISTS`
would hide the drift.

# A room's reach beyond its project (KT-1270)

## Why

One piece of work often spans several repositories: a ticket touching three
projects, an autoCode plan with one sub-task per repository. A room agent's
bridge token is bound to one project (Layer B of
`docs/design/agent-secret-boundary.md`), so the agent of the ticket's room could
neither read nor plan in the other projects. Workarounds (server-side steps that
create the sub-tasks for the agent, copies of other repositories) piled up.

The answer is not a wider token. It is a **room scope**: the token keeps its
project, and a human decides, visibly and revocably, what else the room reaches.

## Model

- **Primary project**: `discussions.project_id`. The working directory, where
  what the room creates lands by default, and the **only** project whose API
  configs and credentials the room may use. Unchanged by any grant.
- **Grants** (`discussion_project_grants`): another project in `read` or
  `write`, or every project (`project_id` NULL, source `all`).
  - `read`: the project's tasks, rooms, runs, pages and workflows read like the
    token's own. Rooms of that project are never written to.
  - `write`: additionally, **planning only**: create tasks whose `project_ids`
    name the project, edit and block its tasks. A task shared with a third
    project needs write on every project it serves. Workflows, pages and quick
    resources of another project stay read-only, so an update can never move
    them into the room's project.
  - every project: `read` like a read grant everywhere; `write` needs the typed
    confirmation `ALL-PROJECTS-WRITE` and the fingerprint of the scope the
    human's screen showed (a stale screen gets a 409).
- **Requests** (`discussion_scope_requests`): an agent of the room asks for a
  project with `room_scope({project, access, reason})`. A request grants
  nothing. The human approves, narrows write to read, or declines. At most five
  pending requests per room; a duplicate stays one request. A name that matches
  no project is filed the same way (and can only be declined), so the reply
  never tells an agent whether a project it cannot read exists.
- **Who reaches a room's grants**: only the room's own agents (the launch of
  that discussion, its room agents), listed at launch in
  `BridgeScope::room_grant_discussions`. A workflow step posting in the room
  (`room_id`), a task worker, or a discussion the token created reach none of
  them.

A grant row is never deleted. Revoking stamps it, and a new grant is a new row;
replacing an access level revokes the old row with reason `replaced`. The table
is the journal; it has no cascading foreign key, so deleting a room or a project
keeps it. Every grant, revoke, request and decline is also a System
`[kronn-scope: …]` line in the room, so the agents and the human see the change
where they work. Agent text in that line (its name for the project, its reason)
loses line breaks and brackets, so it cannot close the line and forge a
decision. The human named in it is the instance's pseudo, never a name from the
request body.

## A workflow's scope (declared at launch)

- A human sets on a workflow the projects its runs reach, read or write
  (planning only), in the workflow page (`workflow_scopes`,
  `PUT /api/workflows/{id}/scope`, not a bridge route). Saving is the
  validation.
- The validation is pinned to the workflow's revision as a run pins it
  (`run_pins::revision_fingerprint`, KT-1096): the workflow and everything it
  executes (Quick Prompts, Quick APIs, sub-workflows, skills, directives,
  profiles). Any later change to any of them, by an agent through the bridge or
  by a human, suspends the scope until a human validates again. A launch card
  shows the scope, or that it is suspended.
- A run is judged on what it pinned, never on the live workflow: its tokens
  reach the scope only when the revision the run pinned at start
  (`pinned_scope_revision`, repository profiles left out) is the validated one.
  Changing a definition, launching, then restoring it never opens the run that
  pinned the change, and a dependency put back after a pin changes nothing for
  that run. A child run, which has no pin of its own, reaches no scope.
- Only a workflow of one project can carry a scope. A global or multi-project
  workflow resolves its references for each launch project, so its runs pin
  another revision than the one validated: the API refuses a scope on it and
  the workflow page says why, rather than showing a validation that would never
  apply.
- A workflow scope may also open **every project** (KT-1336), stored as the
  reserved row `*`: read, or planning write behind the same typed confirmation
  `ALL-PROJECTS-WRITE` and fingerprint of the scope seen as for a room, asked
  again on every validation that keeps it. The fingerprint covers the stored
  rows **and** the workflow's current revision, so a screen read before an edit
  of the workflow is stale (409) and never validates code the human did not
  see. Shown in
  red on the workflow page and the launch card. It follows every rule above
  (pinned revision, suspended by any change, never credentials).
- Withdrawing is always possible (`POST /api/workflows/{id}/scope/revoke`, one
  project, `{"all": true}` for the every-project entry, or `{}` for all), even once a workflow became global: it only removes, never
  validates the remaining entries again, and a run that pinned the validated
  revision loses the withdrawn project on its next call.
- Nothing else gives a step another project.
- Worker launches are unchanged: a task of another project is never executed
  from a token that only holds a grant on it. A worker runs with its task
  project's credentials, which a planning grant must never hand out.

## A room without a project: the general resources (KT-1346)

A room without a project reaches no project. It reads the general resources:
the Quick Prompts, Quick APIs, Pages and workflows with no project, which every
project sees and runs. Writing them is therefore a cross-project write, and is
a human's decision, like every project in write:

- The grant is « General · write » (source `general`, no project, write only).
  A human grants it from the room's Scope settings by typing `GENERAL-WRITE`,
  together with the fingerprint of the scope they saw. The grant is shown in
  red in the room header, recorded in the journal, and revocable. Migration 270
  widens the grants table for it.
- With it, the room's own agents (CLI and HTTP alike) may create general
  Quick Prompts, Quick APIs, Pages and workflows, and edit or delete the ones
  this room created. The gate records that provenance at creation
  (`general_resource_origins`, migration 271), from the token, never from the
  request. Every other general resource stays a human's to edit. Without the
  grant, the bridge refuses with a message that names the grant to ask for.
- A room with a project never gets it: its agents write their project, and a
  creation without a project lands there. A worker, a workflow step or a run
  without a project never holds a room's grant.
- Credentials and API configs stay as before: the grant opens no project's.

## Enforcement

- A refusal on another project names the way out that fits its caller
  (`project_refusal` in `backend/src/core/bridge_token.rs`): `room_scope` for a
  room's own agent, the workflow's scope (workflow page, Projects accessible)
  for a workflow step, the principal for a worker. HTTP agents have the same
  `room_scope` tool as CLI agents; a step or a worker is refused it. The
  workflow guide (`workflow-architect` skill, section Reach) teaches this to
  every agent.

- The token's frozen project does not change (`freeze`). A token whose own
  discussions disagree on their primary project is still refused.
- On every bridge request the gate reads the active grants of the rooms the
  token is the agent of, plus the validated scope of its run's workflow, and
  builds a `Reach` (`backend/src/core/bridge_token.rs`). A failed read refuses
  the request (503), it never widens it.
- `visible_for_read_in`, `writable`, `authorize_in`, `prepare_body_in` and
  `scope_response_in` take that `Reach`; the single-project functions are the
  same code with no grant.
- A project id named by a request: `Read` routes need read; a write route accepts
  another project only under `/api/planning/` and only as a reference (a task's
  `project_ids`), never as what the route changes (so
  `/api/projects/{id}/install-template` stays the token's own project's); effect
  routes always need the token's own project.
- A token reads its own room, run or execution whole, whatever their project.
- No bridge response carries a credential field (`token_override`), on any route.
- MCP configs are judged by the token's own project only, whatever the grants:
  `api_call`, Quick API runs and the plugin overview never see another project's
  configs.
- A project-less token reaches no grant. A room without a project cannot be
  granted one.

## Refused attacks

| Attack | Why it fails |
|---|---|
| The agent grants itself a project | Grant, revoke, approve, decline and workflow-scope routes are not bridge routes: a request carrying a bridge token is refused before the handler. "Human" means a loopback (or operator-authenticated) call without a bridge token, the same trust as the rest of the human UI until per-action human proof lands (Layer B, D1). |
| A step takes a room's grants by pointing its `room_id` at the room (same project or not) | Steps are never the room's agent: `room_grant_discussions` is empty for workflow steps and task workers. |
| An agent edits a workflow whose scope was validated, to use the scope | Any change to the workflow or to what it executes suspends the scope until a human validates again. |
| An agent edits the workflow, launches, then restores it so the scope reads valid | A run reaches the scope only if the revision it pinned is the validated one. |
| An agent edits a Quick Prompt the workflow runs, without touching the workflow | Dependencies are part of the validated revision; the run that pinned the edit reaches nothing, even once the prompt is put back. |
| Write grant used on a project route (`install-template`) | Another project is accepted on a write route only as a planning reference. |
| Learning whether a project exists by asking for it | Unknown names are filed and answered like known ones. |
| Forging an approval line with the reason | Agent text loses line breaks and brackets in the `[kronn-scope: …]` line. |
| Faking who decided | The decider is the instance's pseudo, never a body field. |
| Erasing the journal by deleting the room or the project | No cascading foreign key on the journal. |
| Reading a project's raw `token_override` | Credential fields are stripped from every bridge response. |
| The agent answers its own request through a `kronn-question` | Requests are a separate table decided only by the human routes; a question answer creates no grant. |
| A grant cached after a revoke | Grants are re-read on every request; the next call after a revoke is refused. |
| A workflow scope that can no longer be withdrawn (the workflow became global) | The revoke route is separate from validation and always available. |
| Credential pivot: write grant on p2, then `api_call` with p2's config | Config visibility and the executor scope use the token's own project only. |
| Running p2's workflow from p1 | Effects keep the single-project rule. |
| Pulling p2's workflow into p1 by an update | Write grants open planning only; the update route still forces the token's project and the target is not writable. |
| Every project in write by one click or from a stale screen | Typed confirmation plus the scope fingerprint; any change in between is a 409. |
| Grants spreading to worker rooms or sibling rooms | Grants belong to one discussion; a token reaches the grants of its own discussions only. |
| Flooding the human with requests | Five pending requests per room; duplicates collapse. |

## The grant reaches the agent's filesystem (KT-1406)

The grants above gate Kronn's own tools. A native CLI agent also reads files, so
at every turn the dispatcher reads the room's active grants
(`discussion_scope::room_reach`, from the `projects` table, never from a
request) and builds a plan (`agents/room_reach.rs`):

- **Directory**: each granted project's registered checkout (`read` or `write`;
  an every-project grant covers every registered project but the room's own) is
  validated once by the plan (`VerifiedCheckout::verify`) and handed to the
  launch as a read-only root for Claude Code (`--add-dir`, edits denied). A write
  grant stays read-only on disk; the room's own project is the only one it
  writes. A grant and a revoke apply at the next turn because nothing is cached;
  revoking the last grant leaves the launch without any room policy.
  - **Which path is opened**: a registered path that is, or goes through, a
    symbolic link (or contains `.`/`..`), that is not the root of a Git checkout
    or that overlaps the working directory is not opened, and the agent is told
    its files are not readable in that run. What reaches the launch is the
    canonical root with the device and inode it had at validation, never the
    registered string: the launch (`ReadOnlyRepos::resolve_room`) checks them
    again and refuses the turn if the directory behind the root changed (a link
    to another project, another checkout at the same path, gone). The Git
    metadata of a linked worktree outside the checkout is not opened either, so
    nothing but a registered checkout is ever readable through a grant.
  - **Codex**: a Codex agent keeps its default sandbox: it reads the disk as it
    is (the granted projects included, and every other one) and writes only in
    its working directory. Nothing is opened for it, its launch arguments are
    those it had before KT-1406, and no read isolation is claimed for it: a
    revoke does not close a read for Codex. A profile that denies `:root` was
    tried and dropped: it breaks `git` (the Command Line Tools and
    `~/.gitconfig` become unreadable) and, once the last grant is revoked, the
    room falls back to Codex's wide default, so a room with a grant would be more
    closed than one without. Read isolation for Codex is KT-1409. A workflow's
    `read_only_repos` is unchanged (Codex's `:root` read, only the repositories
    are protected from writes).
  - **Claude Code**: the grant adds exactly the granted checkouts as
    `--add-dir`; with `blockReadsOutsideWorkingDirectories` the file tools read
    nothing else, while shell commands run by Claude's sandbox are not narrowed
    by this policy.
  A task worker, a project-less room, a provider without the policy and a failed
  read of the grants open nothing.
- **Context**: a `ROOM PROJECT SCOPE` block lists the granted projects with
  path and access, and the registered projects that are not granted by name only
  (capped at 40), with the instruction to call `room_scope({project, access,
  reason})` to ask the human. It also tells the agent what to say when a read
  outside its working directories is refused (which project, granted or not, how
  to ask) instead of CLI advice such as `/add-dir` or editing
  `docs/linked-repos.md`. Kronn cannot rewrite the CLI's own refusal text, so
  this explanation is part of the agent's instructions.

## What it does not change

A discussion agent in full access (`full_access`) still runs unrestricted: it can
read on disk whatever the operating system user can read, so no directory is
added for it (narrowing it to the read-only sandbox would be a regression); it
is still told what the room reaches and how to ask. The room scope governs Kronn
objects through the bridge, and, for an agent that is not in full access, which
other checkouts it may read. Reading another repository's code from a workflow
still goes through `read_only_repos` or a copy.

## Tests

- `backend/tests/bridge_token_scope.rs`: read grant, write grant (planning only,
  project routes, credentials and effects refused), revoke on the next call,
  every project with its confirmation, human-only routes, agent request
  lifecycle and its answer for an unknown project, no spreading, steps and
  workers getting no room grant, a workflow scope reaching its runs until the
  workflow changes, tasks shared across projects, `token_override` stripped.
- `backend/src/db/discussion_scope_tests.rs`: journal, replacement, narrowing,
  caps, cross-room decisions, unknown names, forged lines, journal kept on
  deletion; `room_reach` (granted, requestable, every project, revoke, deleted
  project, project-less room).
- `backend/src/agents/room_reach.rs`: grant then next turn has the directory,
  revoke removes it, a write grant is a read-only directory, an ungranted project
  is requestable by name only, no path but a registered Git checkout root is
  opened, a symbolic-link path is never opened and a checkout swapped between
  the plan and the launch is refused, Claude Code gets exactly the granted
  directories as `--add-dir` and none once the last grant is revoked, Codex
  opens nothing (its context block says it reads the disk as it is) and refuses a
  room policy, full-access and unsupported providers open nothing.
  `backend/src/agents/read_only_repos.rs` keeps the workflow Codex profile
  (`:root` read plus the repository entries) under test.

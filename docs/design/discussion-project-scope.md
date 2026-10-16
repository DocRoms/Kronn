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
- The validation is pinned to the digest of what the workflow does (steps,
  trigger, project, variables, guards, failure chain, allow-list): any later
  change, by an agent through the bridge or by a human, suspends the scope until
  a human validates again. A launch card shows the scope, or that it is
  suspended.
- Every token of a run of that workflow (its steps) reaches the validated
  scope, on top of its own project. Nothing else gives a step another project.
- Worker launches are unchanged: a task of another project is never executed
  from a token that only holds a grant on it. A worker runs with its task
  project's credentials, which a planning grant must never hand out.

## Enforcement

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
| An agent edits a workflow whose scope was validated, to use the scope | Any change to the workflow suspends its scope until a human validates again. |
| Write grant used on a project route (`install-template`) | Another project is accepted on a write route only as a planning reference. |
| Learning whether a project exists by asking for it | Unknown names are filed and answered like known ones. |
| Forging an approval line with the reason | Agent text loses line breaks and brackets in the `[kronn-scope: …]` line. |
| Faking who decided | The decider is the instance's pseudo, never a body field. |
| Erasing the journal by deleting the room or the project | No cascading foreign key on the journal. |
| Reading a project's raw `token_override` | Credential fields are stripped from every bridge response. |
| The agent answers its own request through a `kronn-question` | Requests are a separate table decided only by the human routes; a question answer creates no grant. |
| A grant cached after a revoke | Grants are re-read on every request; the next call after a revoke is refused. |
| Credential pivot: write grant on p2, then `api_call` with p2's config | Config visibility and the executor scope use the token's own project only. |
| Running p2's workflow from p1 | Effects keep the single-project rule. |
| Pulling p2's workflow into p1 by an update | Write grants open planning only; the update route still forces the token's project and the target is not writable. |
| Every project in write by one click or from a stale screen | Typed confirmation plus the scope fingerprint; any change in between is a 409. |
| Grants spreading to worker rooms or sibling rooms | Grants belong to one discussion; a token reaches the grants of its own discussions only. |
| Flooding the human with requests | Five pending requests per room; duplicates collapse. |

## What it does not change

A discussion agent still runs with full filesystem access (`full_access`): it can
read on disk whatever the operating system user can read. The room scope governs
Kronn objects through the bridge, not files. Reading another repository's code
from a workflow still goes through `read_only_repos` or a copy.

## Tests

- `backend/tests/bridge_token_scope.rs`: read grant, write grant (planning only,
  project routes, credentials and effects refused), revoke on the next call,
  every project with its confirmation, human-only routes, agent request
  lifecycle and its answer for an unknown project, no spreading, steps and
  workers getting no room grant, a workflow scope reaching its runs until the
  workflow changes, tasks shared across projects, `token_override` stripped.
- `backend/src/db/discussion_scope_tests.rs`: journal, replacement, narrowing,
  caps, cross-room decisions, unknown names, forged lines, journal kept on
  deletion.

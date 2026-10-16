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
  pending requests per room; a duplicate stays one request.

A grant row is never deleted. Revoking stamps it, and a new grant is a new row;
replacing an access level revokes the old row with reason `replaced`. The table
is the journal. Every grant, revoke, request and decline is also a System
`[kronn-scope: …]` line in the room, so the agents and the human see the change
where they work.

## Workflow steps and workers

- A step's `room_id` may name a room of another project when a human granted
  that room write on the run's project. The step's token stays bound to the
  run's project, with the run's credentials; the room is only where it posts.
  A read grant is not enough.
- Worker launches are unchanged: a task of another project is never executed
  from a room that only holds a grant on it. A worker runs with its task
  project's credentials, which a planning grant must never hand out; per-project
  workflows execute their own project's tasks.

## Enforcement

- The token's frozen project does not change (`freeze`). A token whose own
  discussions disagree on their primary project is still refused.
- On every bridge request the gate reads the active grants of the token's own
  discussions and builds a `Reach` (`backend/src/core/bridge_token.rs`). A failed
  read refuses the request (503), it never widens it.
- `visible_for_read_in`, `writable`, `authorize_in`, `prepare_body_in` and
  `scope_response_in` take that `Reach`; the single-project functions are the
  same code with no grant.
- A project id named by a request: `Read` routes need read, `Write`/`Own` routes
  need write, effect routes always need the token's own project.
- MCP configs are judged by the token's own project only, whatever the grants:
  `api_call`, Quick API runs and the plugin overview never see another project's
  configs.
- A project-less token reaches no grant. A room without a project cannot be
  granted one.

## Refused attacks

| Attack | Why it fails |
|---|---|
| The agent grants itself a project | Grant, revoke, approve and decline routes are not bridge routes: a token is refused before the handler. |
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
  credentials and effects refused), revoke on the next call, every project with
  its confirmation, human-only routes, agent request lifecycle, no spreading,
  tasks shared across projects.
- `backend/src/db/discussion_scope_tests.rs`: journal, replacement, narrowing,
  caps, cross-room decisions.

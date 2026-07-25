# Planning and discussion plans

Status: **implemented for 0.9.1** (2026-07-25).

The domain schema, compact HTTP/MCP contract, discussion panel, global backlog,
human-gated proposal cards and delta-only prompt notifications are implemented.
`[src: file: backend/src/db/sql/081_planning_tasks.sql:1-99]`
`[src: file: backend/src/api/planning.rs:1-156]`
`[src: file: backend/scripts/disc-introspection-mcp.py:169-365]`
`[src: file: frontend/src/pages/PlanningPage.tsx:1-650]`
`[src: file: frontend/src/components/DiscussionPlanPanel.tsx:1-300]`

This document is the implementation brief for a future Planning workspace and
the smaller discussion-plan panel that exposes the same task data inside a
conversation. It records the product decisions made during the feature
definition interview. It follows the 0.9.0 release as the 0.9.1 implementation
cycle.
`[src: user: 2026-07-24: Planification feature-definition interview and release roadmap renumbering]`

## Product model

### One task entity

There is one first-class `Task` entity. An idea is a task in the `Idea` status,
not a separate object. A task can be global and initially belong to no project
or discussion. It can later link to zero, one or many projects and discussions.
A cross-project task can carry project-specific subtasks. `[src: user: 2026-07-24: questions 1-5]`

Initial statuses:

- `Idea`
- `Todo`
- `InProgress`
- `Blocked`
- `Done`
- `Archived`

The storage model must allow future statuses such as `Abandoned`; the first UI
only exposes the initial set. Archiving is the default removal action and hard
deletion remains secondary. `[src: user: 2026-07-24: status and archive decisions]`

Initial priorities:

- `Critical`
- `High`
- `Normal` (default)
- `Low`

The global backlog is ranked. Dragging a task across a priority boundary changes
its priority; dragging it within a band changes its rank without changing the
priority. Deadlines are out of scope for the first version. `[src: user: 2026-07-24: priority/backlog decisions]`

### Hierarchy, progress, and definition of done

The database supports arbitrary parent depth, but the initial UI shows only
tasks and subtasks. Subtasks initially inherit their parent's priority but may
diverge. A diverged subtask appears in its own priority band with a parent
breadcrumb. Parent progress is shown as completed subtasks over total
subtasks. `[src: user: 2026-07-24: hierarchy decisions]`

Task details contain:

- title;
- Markdown description;
- a separate Definition of Done checklist, where every item has a sentence;
- repeatable links shaped as `{ label, url }`;
- free-form tags;
- a blocked reason when applicable.

Comments and notes are not part of the first version. Completing every subtask
proposes completion of the parent; it does not complete it automatically.
`Blocked` dependencies use a minimal directed `blocked_by` relation, can cross
projects, expose backlinks, and reject cycles. Finishing all blockers proposes
an unblock action instead of changing status automatically. Dependencies do not
recalculate priorities. `[src: user: 2026-07-24: task-detail and dependency decisions]`

DoD item identifiers remain stable when the checklist is edited. A dedicated
per-item completion endpoint/tool updates one checkbox atomically, so two agents
checking different items do not overwrite one another. Archived and completed
blockers are both treated as satisfied while their relations remain visible for
traceability. Rank writes rebalance the affected active priority band in the
same transaction, preserving a deterministic order after repeated midpoint
inserts.

## Discussion integration

Every discussion can link several tasks and has at most one primary objective.
The same task may be the primary objective of several discussions. Relations
also carry a per-discussion placement:

- `Active`: included in current plan progress;
- `Later`: visible but excluded from current plan progress.

The same task can be active in one discussion and later in another. The
discussion-plan order is independent from global priority. Task status remains
global and therefore stays synchronized everywhere. A primary objective is
necessarily active; moving it to `Later` clears its primary flag.
`[src: user: 2026-07-24: discussion relation decisions]`

The discussion header gets a button such as `Plan · 7/12 +4`. It opens a side
panel using the same interaction pattern as the Git file panel. The panel shows
a vertical timeline: recent completed work, a collapsed “See N completed”
section for the middle, current work, then upcoming work. The linked primary
objective stays visible but collapsible. A small `+` provides quick creation.
Manual additions happen in this panel; no persistent “Add to plan” control is
added to every message. While open, the panel refreshes its compact plan and
selected detail in the background so human and agent edits appear without a
manual close/reopen cycle. `[src: user: 2026-07-24: discussion-panel decisions]`

Agents may instead emit structured proposals that the UI renders as existing
action-like cards. Initial actions are:

- add one or more tasks;
- change a status;
- validate completion;
- unblock;
- open the discussion plan.

When intent is unambiguous an agent may update a task directly through MCP. When
it is ambiguous it should propose an action and leave the click as the human
gate. Agent task edits appear as compact grouped discussion events, including
the acting agent identity. `[src: user: 2026-07-24: agent-action decisions]`

## Global Planning workspace

The first global view is a simple prioritized backlog. It supports:

- search;
- status, project, priority and tag filters;
- with-discussion / without-discussion filtering;
- completed items hidden by default behind a collapsible section;
- direct links to every associated discussion.

Quick creation asks only for title and priority. It defaults to `Idea` in the
global workspace and `Todo` inside a discussion. Full editing opens in a side
panel. Cards stay compact and hide empty metadata; useful visible fields are
title, status, progress, projects, linked discussions, tags and blocked state.
A local, no-token similarity search can suggest possible duplicates. Stable
human references use a format such as `KT-142`. `[src: user: 2026-07-24: global backlog decisions]`

## Agent and MCP contract

Task data is not injected into every agent prompt. Agents receive a compact
change notification only when the discussion plan or a linked task changed,
then pull the required details. This prevents unrelated agents from paying for
unused planning context. `[src: user: 2026-07-24: context-cost decision]`

The intended read surface is:

- `plan_get(discussion_id)` — compact discussion plan plus linked tasks;
- `task_list(filters, cursor)` — compact paginated summaries;
- `task_get(task_id)` — one full task;
- `task_changes(discussion_id, since)` — deltas only.

Writes are separate tools with narrow schemas. Lists are compact by default and
full content is returned only for an explicitly requested task.
`task_update_dod(task_id, dod_id, completed)` is the preferred narrow write for
checkbox progress. Delta timestamps are compared numerically as RFC3339
instants, independent of `Z`/offset spelling and fractional precision. Agents must be
taught this contract in the same prompt/instruction layer that currently
advertises discussion-history tools. Today that layer explicitly describes
`disc_meta`, `disc_get_message`, and `disc_summarize`. `[src: file: backend/src/api/disc_prompts.rs:346-361]`

The first release is local-only. Actor/audit metadata must still distinguish
the human from an agent and record which agent changed a task. Assignment fields
may be reserved in storage but remain absent from the initial UI. A later
delegation flow can create a prefilled discussion from a structured task and
offer both “Create only” and “Create and run”. `[src: user: 2026-07-24: local-first and delegation decisions]`

## Suggested implementation slices

1. **Schema and domain — implemented** — tasks, parent links, ranked priorities, DoD items,
   task links, tags, blockers, discussion relations, actor metadata and event
   log.
2. **Read/write API and MCP — implemented** — compact reads first, explicit writes, cycle and
   authorization guards.
3. **Discussion panel — implemented** — primary objective, active/later timeline, progress,
   creation and agent action cards.
4. **Global workspace — implemented** — prioritized backlog, reorder, filters and task detail
   panel.
5. **Agent behavior — implemented** — instructions, change notifications, grouped activity
   events and source-message provenance.
6. **Project workspace — implemented** — project cards expose their linked
   tasks, quick creation/completion and direct navigation to each task's global
   Planning detail without duplicating task state.
   `[src: file: frontend/src/components/ProjectTasksPanel.tsx:1-200]`
   `[src: file: frontend/src/pages/Dashboard.tsx:1260-1310]`
7. **Deferred delegation** — task-to-discussion briefing and agent launch only
   after the task workflow is proven manually.

## Acceptance anchors

- A task created without a project or discussion is immediately visible in the
  global backlog.
- One task can be linked to multiple projects/discussions while having a single
  global status.
- A task linked to a project discussion is discoverable through that project
  and displays the project badge without a duplicate explicit relation.
- A discussion plan can order active and later work independently from global
  priority.
- Every agent change identifies its actor and is visible without rereading the
  transcript.
- `plan_get` is sufficient for an agent to understand a discussion's active
  objective without fetching the entire task database.
- Dependency cycles are rejected and cross-project blockers remain navigable.
- No plan content is injected when nothing relevant changed.

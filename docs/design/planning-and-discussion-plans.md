# Planning and discussion plans

Status: **validated product design, implementation deferred** (2026-07-24).

This document is the implementation brief for a future Planning workspace and
the smaller discussion-plan panel that exposes the same task data inside a
conversation. It records the product decisions made during the feature
definition interview; it is not an authorization to start the feature before
the delivery sequence below reaches the Planning phase. `[src: user: 2026-07-24: Planification feature-definition interview]`

## Delivery sequence

Current checkpoint:

1. **Complete and merged — quick wins**:
   - bundled PDF/DOCX desktop export and visual fidelity;
   - standard copyable ID pills and feedback for discussions, workflows and
     messages;
   - `disc_get_message` stable-ID lookup plus optional context window;
   - full MCP getters for skills, profiles and directives;
   - shared SQLite/RFC3339 date parsing plus one-shot normalization;
   - Agent+Exec isolation warning and the workflow form-label sweep.
2. **Complete locally — Audit follow-up and release hardening**:
   - crash-safe resume-token rotation;
   - Claude resume binding scoped by logical session and terminal project;
   - repository-wide Rust formatting baseline and enforced CI gate;
   - current Rust clippy findings resolved;
   - complete local backend, frontend, MCP, shell, document-export and Tauri
     quality gates.
3. **Next — push the pre-release branch, run CI, merge, then publish 0.8.13.**
4. **Only after the 0.8.13 release — start the Planning implementation.**

The message-ID work belongs to the quick-win branch because it is a
small, independently useful foundation for future task provenance. The email
share action for generated PDFs is explicitly deferred. `[src: user: 2026-07-24: validated delivery order and deferred email sharing]`

The broader release sequencing and the placement of every remaining TD are
tracked in `docs/design/roadmap-0.8.13-to-planning.md`.

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

## Discussion integration

Every discussion can link several tasks and has at most one primary objective.
The same task may be the primary objective of several discussions. Relations
also carry a per-discussion placement:

- `Active`: included in current plan progress;
- `Later`: visible but excluded from current plan progress.

The same task can be active in one discussion and later in another. The
discussion-plan order is independent from global priority. Task status remains
global and therefore stays synchronized everywhere. `[src: user: 2026-07-24: discussion relation decisions]`

The discussion header gets a button such as `Plan · 7/12 +4`. It opens a side
panel using the same interaction pattern as the Git file panel. The panel shows
a vertical timeline: recent completed work, a collapsed “See N completed”
section for the middle, current work, then upcoming work. The linked primary
objective stays visible but collapsible. A small `+` provides quick creation.
Manual additions happen in this panel; no persistent “Add to plan” control is
added to every message. `[src: user: 2026-07-24: discussion-panel decisions]`

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
full content is returned only for an explicitly requested task. Agents must be
taught this contract in the same prompt/instruction layer that currently
advertises discussion-history tools. Today that layer explicitly describes
`disc_meta`, `disc_get_message`, and `disc_summarize`. `[src: file: backend/src/api/disc_prompts.rs:346-361]`

The first release is local-only. Actor/audit metadata must still distinguish
the human from an agent and record which agent changed a task. Assignment fields
may be reserved in storage but remain absent from the initial UI. A later
delegation flow can create a prefilled discussion from a structured task and
offer both “Create only” and “Create and run”. `[src: user: 2026-07-24: local-first and delegation decisions]`

## Suggested implementation slices

1. **Schema and domain** — tasks, parent links, ranked priorities, DoD items,
   task links, tags, blockers, discussion relations, actor metadata and event
   log.
2. **Read/write API and MCP** — compact reads first, explicit writes, cycle and
   authorization guards.
3. **Discussion panel** — primary objective, active/later timeline, progress,
   creation and agent action cards.
4. **Global workspace** — prioritized backlog, reorder, filters and task detail
   panel.
5. **Agent behavior** — instructions, change notifications, grouped activity
   events and source-message provenance.
6. **Deferred delegation** — task-to-discussion briefing and agent launch only
   after the task workflow is proven manually.

## Acceptance anchors

- A task created without a project or discussion is immediately visible in the
  global backlog.
- One task can be linked to multiple projects/discussions while having a single
  global status.
- A discussion plan can order active and later work independently from global
  priority.
- Every agent change identifies its actor and is visible without rereading the
  transcript.
- `plan_get` is sufficient for an agent to understand a discussion's active
  objective without fetching the entire task database.
- Dependency cycles are rejected and cross-project blockers remain navigable.
- No plan content is injected when nothing relevant changed.

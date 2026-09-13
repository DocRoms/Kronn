# Worker brief: human arbitration and parent milestones

Worker briefs distinguish ordinary implementation judgment from product decisions:
the latter stay with a human. A joined CLI checks durable answers before asking,
publishes a numeric-version `kronn-question` card when needed, verifies its
recording, and pauses only the affected scope. Narrower native and HTTP worker
surfaces request that the principal relay the card; neither actor may decide for
the human. [src: file: backend/src/api/orchestration.rs:5246-5270]

The parent-milestones section is transport-aware, not a single shared string, and
the two joined-CLI tools do not share one restriction. `disc_append` defaults to
this session's own bound sub-discussion but accepts an explicit `disc_id`
argument — it is not restricted to the child room. `task_exec_status` exposes
the real, authorized `parent_discussion_id` (`TaskExecutionLineage`), so a joined
CLI can post an explicit factual milestone (notable progress, a blocker, a
result) straight to the parent room when one is warranted; it never happens
automatically. `disc_question_list`, by contrast, truly is bound-only: it always
reads the runtime-bound `_disc_id()` and ignores any id passed in, so it can
never see the parent room's pending questions. The brief also states that the
orchestrator's attach notice at acceptance and, after a validated
DeliveryManifest, its review request are not the only parent milestones — a
joined CLI may add real ones itself. Native and HTTP workers declare neither
tool at all, so their brief still says so plainly, with no explicit-id nuance to
add. Their relay request is not limited to human decisions: the brief also asks
them to report any other verified notable fact (progress, a blocker, a result)
through their actually available output and to explicitly ask the principal to
relay it toward the parent room — without promising automatic visibility,
guaranteed delivery, or a publication tool the surface does not declare.
[src: file: backend/src/api/orchestration.rs:5271-5300]
[src: file: backend/src/api/orchestration.rs:5136-5157]
[src: file: backend/src/api/orchestration.rs:5163-5189]
[src: file: backend/src/models/orchestration.rs:1196-1203]

The bridge documents that `disc_question_list` reads durable question state and
that `disc_append` is the live discussion publication tool; its implementation
shows `disc_append` accepting an explicit `disc_id` (defaulting to the
runtime-bound one) while `disc_question_list` always resolves `_disc_id()` and
ignores any id argument.
[src: file: backend/scripts/disc-introspection-mcp.py:176-177]
[src: file: backend/scripts/disc-introspection-mcp.py:5063]
[src: file: backend/scripts/disc-introspection-mcp.py:4278-4289]

# Worker brief: human arbitration and parent milestones

Worker briefs distinguish ordinary implementation judgment from product decisions:
the latter stay with a human. A joined CLI checks durable answers before asking,
publishes a numeric-version `kronn-question` card when needed, verifies its
recording, and pauses only the affected scope. Narrower native and HTTP worker
surfaces request that the principal relay the card; neither actor may decide for
the human. [src: file: backend/src/api/orchestration.rs:7629-7654#human_arbitration]

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
[src: file: backend/src/api/orchestration.rs:7655-7684#parent_milestones]
[src: file: backend/src/db/orchestration.rs:5540-5576#get_execution_lineage]
[src: file: backend/src/api/orchestration.rs:15429-15433#worker_briefs_make_human_arbitration_and_parent_milestones_transport_aware]
[src: file: backend/src/models/orchestration.rs:1324-1337#TaskExecutionLineage]

The bridge documents that `disc_question_list` reads durable question state and
that `disc_append` is the live discussion publication tool; its implementation
shows `disc_append` accepting an explicit `disc_id` (defaulting to the
runtime-bound one) while `disc_question_list` always resolves `_disc_id()` and
ignores any id argument.
[src: file: backend/scripts/disc-introspection-mcp.py:221-222#disc_question_list]
[src: file: backend/scripts/disc-introspection-mcp.py:5377-5395#call_disc_append]
[src: file: backend/scripts/disc-introspection-mcp.py:4572-4576#call_disc_question_list]

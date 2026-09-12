# Worker brief: human arbitration and parent milestones

Worker briefs distinguish ordinary implementation judgment from product decisions:
the latter stay with a human. A joined CLI checks durable answers before asking,
publishes a numeric-version `kronn-question` card when needed, verifies its
recording, and pauses only the affected scope. Narrower native and HTTP worker
surfaces request that the principal relay the card; neither actor may decide for
the human. [src: file: backend/src/api/orchestration.rs:5246-5270]

The parent-milestones section is transport-aware, not a single shared string: a
joined CLI is rebound into its own sub-discussion at acceptance and is no longer
present in the origin room, so its `disc_append`/`disc_question_list` calls only
ever reach that sub-discussion — never the parent room directly. Its brief states
that scoped fact, not a blanket "no publication tool" claim, which would
contradict the arbitration section naming those same tools right above it. Native
and HTTP workers declare no publication tool at all, so their brief says so
plainly. Both variants name the same real parent-room facts: the orchestrator's
attach notice at acceptance and, after a validated DeliveryManifest, its review
request — no automatic mirror is added.
[src: file: backend/src/api/orchestration.rs:5271-5295]
[src: file: backend/src/api/orchestration.rs:5136-5157]
[src: file: backend/src/api/orchestration.rs:5163-5189]

The bridge documents that `disc_question_list` reads durable question state and
that `disc_append` is the live discussion publication tool.
[src: file: scripts/disc-introspection-mcp.py:176-177]
[src: file: scripts/disc-introspection-mcp.py:5045-5053]

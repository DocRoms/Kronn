# TD-20260724-planning-and-discussion-plans

- **ID**: TD-20260724-planning-and-discussion-plans
- **Area**: Backend / Frontend / MCP / Discussions
- **Problem (fact)**: Kronn discussions currently have messages, files, Git
  changes and agent activity, but no shared structured plan. Work, ideas,
  priorities and completion state therefore stay embedded in transcripts or are
  repeated across discussions. Agents cannot retrieve or update a common task
  view through `kronn-internal`; the current introspection family only exposes
  discussion metadata, messages and summaries. `[src: file: docs/operations/mcp-servers/kronn-internal.md:9-17]`
- **Why we can't fix now (constraint)**: the product shape is validated, but
  this cross-cutting feature is intentionally deferred until its implementation
  work starts as a dedicated change.
- **Impact**: product usability | agent context cost | cross-discussion
  traceability | prioritization
- **Where (pointers)**:
  - validated design: `docs/design/planning-and-discussion-plans.md`;
  - discussions UI: `frontend/src/pages/DiscussionsPage.tsx`,
    `frontend/src/components/ChatHeader.tsx`;
  - discussion MCP bridge: `backend/scripts/disc-introspection-mcp.py`;
  - agent history contract: `backend/src/api/disc_prompts.rs`;
  - current message introspection endpoint:
    `backend/src/api/disc_introspection.rs`.
- **Suggested direction (non-binding)**: implement one global task entity with
  extensible status, ranked priority, subtasks, DoD checklist, free links/tags,
  cross-project blockers and discussion relations (`primary`, `active`,
  `later`). Build compact MCP reads before agent writes; use one global backlog
  plus a Git-like discussion side panel over the same data. Keep automatic
  prompt injection delta-only.
- **Next step**: turn the design's implementation slices into tickets and begin
  with the domain schema plus compact read API.

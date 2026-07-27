# Discussion portability

Kronn exports one discussion as a self-contained JSON document with
`kind: "kronn.discussion"` and schema `version: 1`. The bundle includes the
transcript, message authors and model metadata, attachment bytes when Kronn
still has them (otherwise the extracted text), message-revision audit events,
and the tasks directly attached to the discussion plan.
[src: file: backend/src/api/disc_portability.rs:36-114]
[src: file: backend/src/api/disc_portability.rs:189-289]

## Secret boundary

Conversation and attachment content is intentionally included and the export UI
warns about that scope. Kronn excludes runtime credentials, explicit external
CLI session ownership, local workspace/worktree paths, sharing identifiers,
workflow-run linkage and partial execution state. This policy is written into
every bundle as `secret_policy` so a receiver can inspect it before import.
[src: file: backend/src/api/disc_portability.rs:38-39]
[src: file: backend/src/api/disc_portability.rs:291-329]

## Import and conflicts

Import creates fresh local discussion, message, attachment and planning-task
identities. Source message ids are retained as lineage, and references in
attachments, revision events and task events are remapped to those fresh ids.
Unknown projects and relationships outside the exported plan become explicit
warnings instead of dangling foreign keys.
[src: file: backend/src/api/disc_portability.rs:439-764]

The `discussion_imports` ledger stores the source discussion id, a semantic
SHA-256 of the bundle and the created local discussion. Replaying the same
bundle returns that discussion without duplicating data. Reusing the source id
with changed content returns a conflict and does not mutate the existing copy.
[src: file: backend/src/db/sql/093_discussion_imports.sql:1-15]
[src: file: backend/src/api/disc_portability.rs:445-472]

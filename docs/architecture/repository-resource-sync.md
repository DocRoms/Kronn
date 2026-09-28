# Repository resource synchronization

Repository resources are materialized only by an explicit publish operation. That operation writes the selected resource together with the generated index, configuration, router skill, lock file, and the `docs/AGENTS.md` pointer; it checks ownership before replacing existing files. [src: file: backend/src/core/repository_resources.rs:650]

Imports accept only relative paths recorded in `kronn.lock` and reject symlink escapes before reading a definition. [src: file: backend/src/core/repository_resources.rs:436]

Alignment is hash-based. The stored repository and database baselines distinguish aligned, repository-modified, Kronn-modified, and conflict states; conflicts expose a diff without automatically choosing a side. [src: file: backend/src/api/projects/resources.rs:173]

Alignment baselines and approvals are local SQLite state rather than repository files. Approvals are keyed by project, resource kind, slug, and content hash, so a changed executable definition requires a new approval. [src: file: backend/src/db/sql/200_repository_resource_alignment.sql:4] [src: file: backend/src/db/sql/200_repository_resource_alignment.sql:19]

The executable fingerprint excludes local identifiers, timestamps, favorites, and workflow activation state while retaining the portable definition. [src: file: backend/src/core/repository_resources.rs:95]

Imported workflows, Quick Prompts, Quick APIs, and Quick Execs share the same fingerprint-bound execution check. Standalone launches report approval failures as preflight failures, while workflow references are checked while hydrating the saved resource before any agent or plugin execution. [src: file: backend/src/core/repository_resources.rs:731-817] [src: file: backend/src/api/quick_apis.rs:421-453] [src: file: backend/src/api/mcp_remote.rs:442-454] [src: file: backend/src/workflows/quick_prompt_hydrate.rs:43-63] [src: file: backend/src/workflows/quick_api_hydrate.rs:45-63]

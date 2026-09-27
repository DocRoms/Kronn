# Custom skill/directive/profile identity (ADR-005 slice 1)

Custom skills, directives and profiles are Markdown+frontmatter files under
`~/.config/kronn/{skills,directives,profiles}/`. Each one's id (`custom-<slug>`)
is derived from the **filename**, not stored inside the file
[src: file: `backend/src/core/skills.rs`].

## Stable id, in-place update

`update_custom_skill` / `update_custom_directive` / `update_custom_profile`
locate the file from the id passed in and overwrite it in place — the slug is
never recomputed from a new display name, so renaming a resource never
changes what `skill_ids` / `directive_ids` / `profile_ids` must reference.
The write goes through `core::mcp_scanner::atomic_write` (temp file + rename),
same as `save_custom_*` on create.

`api/skills.rs`, `api/directives.rs`, `api/profiles.rs`'s `PUT` handlers call
these instead of the old delete-then-recreate pattern.

## Slug collisions

`unique_skill_slug` / `unique_directive_slug` / `unique_profile_slug` (create
path only) append `-2`, `-3`, … until they find a filename that doesn't
already exist, so two different display names that produce the same slug
(e.g. "Foo Bar" and "foo-bar") never overwrite each other.

## Run snapshot

`core::resource_snapshot::RunSnapshotCache<T>` pins each `(run_id, resource_id)`
pair to the value first resolved under that `run_id`; a later edit or
deletion on disk doesn't change what that run already loaded.
`get_skills_snapshot` / `get_directives_snapshot` / `get_profile_snapshot` and
the `build_*_prompt_for_run` variants are the snapshot-aware equivalents of
the always-fresh `get_*_by_ids` / `build_*_prompt` functions.

`agents::runner::AgentStartConfig.run_snapshot_id` selects between the two:
`Some(run_id)` resolves through the snapshot, `None` (the default) keeps the
previous always-fresh behaviour. Workflow step execution
(`workflows::steps::execute_step`, threaded down into `run_agent_with_timeout`
and `run_multi_agent_debate`) passes `Some(&WorkflowRun.id)`, so every step,
repair, escalation and multi-agent-debate turn of the SAME run sees the same
resource versions. `workflows::runner::RunSnapshotGuard` (inserted at the top
of `execute_run_with_notify_policy`, alongside the existing `CancelGuard`)
releases that run's pinned entries when the call returns — including a Gate
pause: `resume_run` re-enters through `execute_run`, so a paused-then-resumed
run re-snapshots fresh at resume time rather than staying pinned across an
unbounded approval wait.

Known gap: `core::native_files::sync_project_native_files` (writes the actual
`SKILL.md` / agent files a CLI discovers on disk) still resolves live, not
through the snapshot — only the prompt-injected content is pinned. Low-stakes
in practice: prompt injection is the channel present for every agent
(native discovery is an additional, CLI-specific path), but a future slice
should close this gap if native-only agents need the same guarantee.

The ad-hoc "Test step" preview endpoint (`api/workflows.rs`, no persisted
`WorkflowRun`) passes `None` — each preview call should see current disk
content, not a stale pin from an earlier unrelated call.

# Resource identity table and symbolic references (ADR-005 slice 3)

`resource_identities` (migration 196) generalises the Artifact-bundle-only
`artifact_import_origins` (migration 188) into `(project_key, kind, slug) ->
target_id`, with at most one target per key. See
`backend/src/db/sql/196_resource_identities.sql` and
`backend/src/db/resource_identities.rs`.

- `project_key` is the destination project's normalised `repo_url`
  (`api::discover::normalize_repo_url`), else its path, else `""` — never the
  local project UUID, which does not survive a clone onto another machine.
  Limitation: the key is computed at lookup time from the project's current
  `repo_url`/path, not stored on the project — a project that gains a
  `repo_url` after some identities were recorded under its former path-based
  key no longer matches those rows (they stay reachable only through the
  global-scope fallback below, keyed on `slug` alone).
- `slug` is an Artifact's real, declared `slug` field; for Workflow, Quick
  Prompt, Quick Api and Quick Exec — which don't carry a separate slug field
  yet — it is the bundle's own `id`, the identity already stable across
  re-exports of the same resource (`Resource::slug` in
  `backend/src/api/artifact_portability/import.rs`).
- `resource_identities::upsert` is an `INSERT ... ON CONFLICT DO UPDATE`:
  reimporting the same `(project_key, kind, slug)` updates that one row
  instead of accumulating a duplicate mapping, unlike the table it replaces.

See also [resource-library-identity](resource-library-identity.md) (ADR-005
slice 1, custom skill / directive / profile identity) for the companion,
filename-derived identity that predates this table and that this slice's
symbolic references resolve against.

## Candidate matching in the Artifact bundle importer

`artifact_portability::import::prepare_plan` looks up
`resource_identities::lookup_scoped(project_key, kind, slug)` to find a
prior local copy of a bundled dependency, replacing the former unscoped
`(kind, source_id)` scan. `lookup_scoped` tries `project_key` first, falling
back to the global scope (`""`) when it is non-empty and the project-scoped
row misses — the same rule `resolve_symbolic_reference` uses, factored into
one function so both call sites and the migration's global-scope rows agree.
This matters because migration 196 placed every identity that predates this
table into the global scope: without the fallback, importing a
previously-imported slug into a project would never find that row and would
create a duplicate copy instead of reusing it. Two imports of the same
bundle into two different projects still don't share or collide on *new*
identities — each project's own import still records its own
project-scoped row (`record_origin`, called only for a `Create`
disposition). The root Artifact keeps its existing "always a fresh copy"
behaviour (`artifact_roundtrip_preserves_null_points_and_reuses_previous_import_identities`);
only the anti-stale-preview observation changed, from a growing "generation"
counter to observing whichever identity the slug currently maps to
(`prepare_plan` in `backend/src/api/artifact_portability/import.rs`).

## Symbolic references

`resource_identities::resolve_symbolic_reference` resolves `prompt:<slug>`,
`workflow:<slug>`, `qe:<slug>`, `qa:<slug>`, `skill:<slug>` and
`plugin:<server>` to a local identifier, using the same `lookup_scoped`
project-then-global rule described above. `plugin:<server>` reuses
`mcps::find_config_for_server`; `skill:<slug>` tries `custom-<slug>` first
(ADR-005 slice 1 ids) then the bare slug (builtin skills, embedded at
compile time, e.g. `rust`).

## Instance-bound requirements reused, not reinvented

The Artifact bundle importer already reused `remap_html`
(`artifact_portability::import::remap_html`) for ids embedded in HTML
actions. This slice adds the same reuse for `rebind_api_configs` /
`rebind_quick_api_config` — previously only called from the legacy
`/api/workflows/import` endpoint — so a newly created Workflow or Quick Api
also retargets a dangling `api_config_id` to a local config for the same
plugin (`api_plugin_slug`) when one exists (`commit_plan` in
`backend/src/api/artifact_portability/import.rs`, calling
`rebind_api_configs` in `backend/src/api/workflows.rs`).

A newly created Workflow step's `skill_ids` / `profile_ids` / `directive_ids`
are filtered through `core::skills::get_skill` / `core::profiles::get_profile`
/ `core::directives::get_directive` at commit time
(`resolve_step_requirements`): a reference that resolves locally is kept
unchanged (ids already double as slugs, per ADR-005 slice 1), one that
doesn't is dropped rather than carried over dangling. `execution_requirements`
already reports the same gap as a non-blocking `ArtifactImportWarning` in the
preview, for every kind (skill, profile, directive, model connection, plugin
connection) — missing instance-bound requirements are signalled, never
silently kept or blocked on.

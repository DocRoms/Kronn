# Portable `.agents` library operations

Kronn's portable library combines global resources from the Kronn configuration
directory with project resources from the current checkout. Project resources
override global resources with the same kind and id. `kronn sync` materializes
the effective catalog under the project's `.agents/` directory and writes a
deterministic lock file. [src: file: backend/src/core/portable_library.rs:321-352]
[src: file: backend/src/core/portable_library.rs:869-878]

## Bootstrap and sync

Run commands from the project root:

```sh
kronn sync
kronn check --frozen-hash
```

The sync report separates created, modified, deleted, and unchanged managed
paths. Sync only deletes paths recorded by the previous managed manifest;
unmanaged project files and skill auxiliary resources are preserved.
[src: file: backend/src/main.rs:15-25]
[src: file: backend/src/core/portable_library.rs:578-704]

Re-running sync with unchanged inputs is a no-op. A project-local item wins over
the global item with the same `(kind, id)`, while a managed global copy in the
project tree does not accidentally become a project override.
[src: file: backend/src/core/portable_library.rs:321-352]
[src: file: backend/src/core/portable_library.rs:2664-2780]

## Library layout and provenance

The portable tree uses these layouts:

- Skills: `.agents/skills/<id>/SKILL.md` with an adjacent
  `SKILL.kronn.json` sidecar.
- Quick Prompts: the same Agent Skill layout, with the complete legacy binding
  and typed inputs in the sidecar.
- Directives: `.agents/directives/<id>.kronn.json`.
- Workflows: `.agents/workflows/<id>.kronn.json`.

Every sidecar records kind, id, origin scope, portable source path, and a
SHA-256 content digest. The human-readable `SKILL.md` remains an ordinary Agent
Skill; Kronn metadata is kept out of its frontmatter.
[src: file: backend/src/core/portable_library.rs:101-137]
[src: file: backend/src/core/portable_library.rs:879-890]

## Frozen checks and approval

Use the frozen check in CI and pre-commit validation:

```sh
kronn check --frozen-hash
```

It refuses added, removed, or altered files relative to `.agents/kronn.lock`.
Approval is a separate trust-on-first-use action:

```sh
kronn check --approve
```

Approval is bound to both the canonical project and the current lock digest.
It becomes stale after a reviewed lock change and cannot be replayed into an
otherwise identical checkout. Approval data is stored outside the project so
it is not committed or synced as library content.
[src: file: backend/src/core/portable_library.rs:705-856]
[src: file: backend/src/core/portable_library.rs:2521-2615]

## Workflow check, variables, and execution

Validate and render variables without executing:

```sh
kronn run .agents/workflows/<id>.kronn.json --check --var name=value
```

Generate the secret-free environment template:

```sh
kronn run .agents/workflows/<id>.kronn.json --render-env
sh .agents/scripts/render-env.sh .agents/workflows/<id>.kronn.json
```

Execution requires explicit opt-in:

```sh
kronn run .agents/workflows/<id>.kronn.json --allow-exec --var name=value
```

The runner uses literal executable/argument arrays rather than a shell. A
workflow must declare each executable in `requires`, variables are rendered in
one pass, and execution requires a frozen lock plus a current approval.
[src: file: backend/src/core/portable_library.rs:1251-1409]
[src: file: backend/src/core/portable_library.rs:1411-1587]

Secrets are referenced through `${env:NAME}` declarations. Portable documents
containing credential-shaped values are rejected; generated `.env.example`
files contain names/placeholders, not resolved secret values.
[src: file: backend/src/core/portable_library.rs:16-75]
[src: file: backend/src/core/portable_library.rs:1397-1409]

## Container fallback

When the native binary is unavailable, mount the checkout and run validation
with the release image that matches the project lock:

```sh
docker run --rm -v "$PWD:/workspace" -w /workspace \
  kronn:<release> kronn run .agents/workflows/<id>.kronn.json \
  --check --var name=value
```

Pin `<release>` explicitly. Do not use a floating image tag for frozen checks.

## Migration and rollback

Quick Prompt migration preserves the complete existing `QuickPrompt` value in
the sidecar, including its bindings and variables. The editable `SKILL.md` body
is authoritative when importing it back. Legacy Quick Prompt JSON is accepted
until the skill layout has been materialized; afterward the skill wins, making
repeated migration idempotent.
[src: file: backend/src/core/portable_library.rs:478-537]
[src: file: backend/src/core/portable_library.rs:959-1051]

For any library migration:

1. Commit or copy the existing source files before the first sync.
2. Run `kronn sync` and inspect `.agents/kronn.lock` plus the reported changes.
3. Run `kronn check --frozen-hash`.
4. Review the diff, then run `kronn check --approve` only if the lock is trusted.
5. Commit the portable resources and lock together.

Rollback by restoring the previous committed `.agents` tree and lock, running
the frozen check, then approving the restored lock for that checkout. Do not
copy approval state between projects.

## Error recovery

- **Frozen check reports `added`, `removed`, or `altered`:** inspect the named
  path. Re-run sync only when the source catalog is authoritative; otherwise
  restore the committed file.
- **Approval required or stale:** review the lock diff, then run
  `kronn check --approve`. Never automate approval in CI.
- **Duplicate kind/id:** remove or rename the duplicate in the same scope.
  Filesystem enumeration order is deliberately not used to pick a winner.
- **Secret rejected:** replace the literal with an `${env:NAME}` workflow
  reference and keep the value outside `.agents`.
- **Missing executable:** install it and declare it in `requires`; do not wrap
  it in a shell command.
- **Interrupted sync:** run sync again. Managed output and lock generation are
  deterministic, so an unchanged catalog converges to the same tree.


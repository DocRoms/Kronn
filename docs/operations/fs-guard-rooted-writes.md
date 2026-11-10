# fs_guard: rooted writes and the Windows residual risk

Status: KT-1595. Code: `backend/src/core/fs_guard.rs` (the `guarded_*` writes) on
`backend/src/core/rooted_fd.rs` (the directory-fd primitive, shared with the run
artifacts root of KT-1585).

## What is guaranteed

On unix every `guarded_*` write resolves through directory file descriptors. The
root is opened once (the root itself is a path Kronn chose and may be reached
through links), then each component below it is created with `mkdirat` and opened
with `openat(O_NOFOLLOW | O_DIRECTORY)` from the previous descriptor. File names
are only resolved against the held parent descriptor (`openat`, `linkat`,
`renameat`, `unlinkat`). A symlink below the root is never followed, whether it
was there before the call or was swapped in for a parent between the check and
the write.

| Operation | What it does |
|---|---|
| `guarded_create_dir_all` | `mkdirat` + `openat(O_NOFOLLOW)` per component |
| `guarded_write_new`, `guarded_copy_new` | temp sibling, published no-clobber with `linkat`; the temp's device and inode are compared with its name right before publishing |
| `guarded_replace` | temp sibling, `renameat` over the name (a link planted after the check is replaced, never written through). A regular file keeps its mode; a new file gets 0666 under the umask, as a plain write would |
| `guarded_replace_if_unchanged_keeping_mode` | the same, refused when the file was replaced or its bytes changed since they were read, with the file's mode kept. One handle is held; the check is optimistic, not an atomic compare-and-swap (see below) |
| `guarded_replace_like` | `guarded_replace` with the data synced before the `renameat`; an entry that is not a regular file (a link included) is refused |
| `guarded_set_mode` | `fchmod` through an `O_NOFOLLOW` handle |
| `guarded_remove_file`, `guarded_remove_dir`, `guarded_remove_tree` | `unlinkat`; the tree walk opens each level `O_NOFOLLOW` |
| `guarded_rename` | `renameat` between two held parents |
| `guarded_move_new` | opens the source `O_NOFOLLOW`, `linkat` no-clobber, then `unlinkat` of the source only while its name still is the opened file (device and inode) |
| `guarded_write_new_identified`, `guarded_remove_file_if_ours` | a create that returns the file's identity, and a removal that checks the name still is that file; a stat then an `unlinkat`, not an atomic compare-and-unlink |
| `guarded_read` | reads a regular file through a handle opened `O_NOFOLLOW` against the held parent; a link is refused |

Written content carries the `rwx` bits only (`0o777`): setuid, setgid and sticky
are never kept from an existing file nor copied from a template.

A parent that is *moved* (not replaced by a link) inside the root during the call
still receives the write: the descriptor holds the original folder, which is
under the root.

### Detection, not prevention

The temp file's device and inode are compared with its name right before it is
published, and the published entry is compared with the temp right after. A temp
swapped for a link or another file in the instant between the first comparison and
the `linkat`/`renameat` is therefore detected after the fact and the call fails.
Nothing is unlinked by name after the publication: the entry could by then be a
concurrent writer's legitimate file, or our own good one, so the mismatch is only
reported and the entry is left where it is. A link or foreign file published in
that instant therefore stays inside the root. Nothing outside the root is ever
written, because every step runs against the held descriptors.

`guarded_replace_if_unchanged_keeping_mode` is optimistic: right before the
`renameat` it checks that the name is still the file it opened (device and inode)
and that the bytes read again through that handle are the ones observed. A write
landing after that check is lost; it is not an atomic compare-and-swap.

## Callers

Every project write that checks a path with `assert_contained_no_symlink` (or gets
it from `docs_location::guard_write`) now uses a `guarded_*` function: native
files, step guard, repository resources, skill migration, operating seed,
`briefing` / `briefing_rules` / `project_doc` / `cross_audit` / `helpers`
(audit), `template_reinstall` (publish, backups, rollback), `template_manifest`,
`cross_audit_decision`, `doc_provenance`, `docs_migration`, `root_agent_files`,
`scanner`, `legacy_docs`, `kronn_state` (the merge state is anchored at the merge
worktree, not at its docs folder), `audit_run_state`, the template install
(`copy_dir_nondestructive_in`, reached from `install_template`, the project
bootstrap and the Full audit, with the linked-repos doc it writes), the
`docs_migration` reference rewrites, its `ai/` to `docs/` merge and its
`index.md` to `AGENTS.md` rename (rooted at the project, so a link already
committed in `docs/` is refused with no race needed),
`legacy_docs` (rooted at the project, not at its docs folder), and, in `worktree`,
the lane target, the managed worktree folders and the `.git` / `gitdir` files
`fix_worktree_paths` rewrites (rooted at the worktree and at the repo's `.git`). A `template_reinstall` `Create` is create-only (a file
that appeared since the plan is never overwritten nor deleted by a rollback) and
its rollback is exact: it only claims the files it really put back, removes a
created file only while its name still is the file it created, and decides what
to restore from a rooted read, never a path read.

Checked and read-only (the check is only a refusal, nothing is written by path
afterwards): `ci_facts`, `stack_consistency`, `doc_commands`, `audit_baseline`,
`briefing_docs`, `approved_scripts`, `git_ops`, `docs_folder` (its move goes
through `legacy_docs`), `resources` reads, `repository_resources` reads,
`skill_migration` scans, `template_manifest` checks, `rooted_io` (its own
descriptor-based primitive on unix).

Each migrated family has a test that swaps the parent folder for a link after the
caller's own checks (`core::fs_guard::swap_probe`) and proves nothing lands
outside the root.

## What is not covered (any platform)

- `assert_contained_no_symlink` alone is an lstat check, true at one instant. A
  caller that checks and then writes the path itself is not covered: use a
  `guarded_*` write.
- Reads (`resolve_contained_read` canonicalizes; a swap after it can be read
  through).
- Files a spawned `git` or an agent writes by path (the `git mv` of
  `docs_migration` is one: its paths are checked first and its fallback is rooted).
- Other by-path writers that never call `fs_guard`: `root_agent_files::inject_or_update`,
  `td_decisions`, `adr`, `skills`, `profiles`, and in `worktree` the copy of the
  untracked files into a new checkout, the `info/exclude` append and the
  submodule parent folders.
- `template_reinstall` applies the template's file renames before it writes
  (`apply_renames_in`); a later failure rolls back the files it wrote, not those
  renames.

## Windows: residual risk, accepted

There is no directory-descriptor primitive in use on Windows. There, every
`guarded_*` write keeps the behaviour it had before KT-1595: the lstat walk, then
the write by path. A parent folder swapped for a link between the walk and the
write is therefore not excluded. Creating a symlink on Windows needs elevated
rights or developer mode, which is why this is accepted rather than failing
closed: worktree creation, template installs, native files and seeding must keep
working on the desktop build. The owner decided this and amended DoD item 3 of
KT-1595 accordingly (item 3 as written asked every target to fail closed).

Closing it needs handle-rooted file operations on Windows. That is the follow-up
ticket **KT-1609** (Windows rooted primitive); not done here.

Any target that is neither unix nor Windows fails closed: every `guarded_*` write
is refused and nothing is created (the `cfg(not(any(unix, windows)))` module in
`core/fs_guard.rs`).

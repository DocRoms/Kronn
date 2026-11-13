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
| `guarded_list` | lists a folder through the held descriptors (`fdopendir` on a duplicate); a link at the folder or in a parent is refused (an error), never listed as empty; a missing folder is `None`; a read error is an error |
| `guarded_create_new_dir` | `mkdirat` that fails on an existing entry (a folder, a file or a link): `false` when the name is occupied, never adopted. The idempotent `guarded_create_dir_all` stays for callers that want that |

A read-like refusal of the walk (a link at the name or in a parent) is a `PermissionDenied` error carrying a typed payload: `fs_guard::is_link_refusal(&error)` recognizes it (and the `ELOOP` of the `O_NOFOLLOW` open), so a caller can skip one planted entry without swallowing a real `EACCES`.

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
`legacy_docs` (rooted at the project, not at its docs folder), the writers that run
right after an agent has been in the project (the second pass of KT-1595):
`briefing` completion (`complete_reviewed_briefing` reads and restores
`docs/briefing.md` against the project root threaded from the stream), the
`redact_artifacts` sweep (a rooted read, then a compare-and-replace that keeps the
mode), `validation` (`settle_unaddressed_signals`, `repair_coverage_rows`),
`document_repair` (the marker demotion and the exact repairs),
`revalidate::apply_reanchors`, the rollback snapshot of `validate_version`,
the `citation_drift` record, the `checksums` baseline (publish, rollback and
`acknowledge_sections`) and the TD decisions of
`audit_validation::apply_td_resolutions_at` with their rollback, the writers that
run on a project's docs folder after the post-run sweep, each now taking the
project root next to the docs folder (`td_decisions::restore(root, docs_dir, …)`,
`adr::settle_covered_sheets(root, docs_dir, …)`, `td_index::resync(root,
docs_dir, …)`, which refuse a `docs_dir` outside `root`) and the reconciliation
report of the Full audit, the audit state folder `.kronn/audit` (`audit_state`:
folder creation, create-new stores, removals, the legacy migration and
`write_state_file`, used by the documentary-optimization report and the blame
record of `document_repair`), and, in `worktree`,
the lane target, the managed worktree folders and the `.git` / `gitdir` files
`fix_worktree_paths` rewrites (rooted at the worktree and at the repo's `.git`). A `template_reinstall` `Create` is create-only (a file
that appeared since the plan is never overwritten nor deleted by a rollback) and
its rollback is exact: it only claims the files it really put back, removes a
created file only while its name still is the file it created, and decides what
to restore from a rooted read, never a path read.

The third pass of KT-1595 (post-agent writers that still wrote project documents
by path, found by the round-3 review and the Argon addendum):

- `validation::fill_step_owned_placeholders` (the `{{DATE}}` fill, run right after a
  CLI succeeds by the Full audit and the drift refresh): `guarded_read`, then
  `guarded_replace_if_unchanged_keeping_mode` under the project root. A link at the
  target name or in a parent is refused and logged.
- `anti_hallu_enforce::stamp_target_audit_dates` (the audit-date stamp of enforce
  mode, in `full.rs` and `drift.rs`): the same read and optimistic replace.
- `step_remit` (the remit guard of KT-1360, `settle` after every attempt): the
  capture of a dirty file, the "unchanged" comparison, the blob kept before a restore
  (the bytes are read through the primitive and handed to `git hash-object --stdin`,
  git never reads the path) and the restore itself (`guarded_replace`,
  `guarded_write_new` when the file is gone, `guarded_remove_file` for a file that was
  absent before the step). An AI-doc folder swapped for a link, or a link planted at
  the tracked name, is refused and reported as a warning ("could not be restored");
  nothing is written or deleted through it. A file that could not be read at capture
  (a link) is never restored.
- `anti_hallu_step::apply` (step 0 of every audit, resumes included, and the inject
  route): `guarded_read`, then an optimistic replace. A link at `AGENTS.md` or in a
  parent is an error, not "file missing".
- `td_index::resync` (the central index rebuilt from the sheets): the index, the
  listing of `tech-debt/`, every sheet and the dimension indexes are read through
  `guarded_list` / `guarded_read` under the project, and the central index and the
  dimension indexes are read once, so the rows and the rendering come from the same
  bytes. A refused listing or read (a link at `tech-debt/`, at one sheet, at the
  index, or in a parent) is an ERROR that skips the resync; it is never "zero
  sheets", which used to rewrite a real index as if every sheet had been removed. The
  read-only views of the same module (cards, reviews) stay by path.
- `td_decisions::restore`: a link planted at a decided sheet's name (the walk refuses
  it with `PermissionDenied`, not `ELOOP`) skips that sheet and is reported
  (`RestoreOutcome::refused`, the `td_decisions` event), and the other decisions are
  still put back; a `tech-debt/` folder that is itself a link still aborts. A deleted
  sheet is written back create-only (`guarded_write_new`): an entry that appeared
  since the read is left alone and reported. The listing is `guarded_list`.
- `adr::settle_covered_sheets`: no check by path before the read of `decisions.md`;
  the guarded read decides (missing is no decision, a link or an escape is an error).
- `worktree::ensure_local_git_exclude` (reached from `audit_state::state_dir` on every
  state write): rooted at the git directory git names (`<git dir>/info/exclude`); the
  read and the write (create-only when the file is missing, an optimistic replace
  otherwise) refuse a link at `exclude`, at `info` or in a parent. The write is a
  replace by rename, no longer an append: a line another process appends in that
  instant is refused, not lost silently.
- `audit_state::new_template_backup_dir`: `guarded_create_new_dir` per candidate name,
  the suffix advances on an occupied name; a folder that appears between the check and
  the create is never adopted.
- The reconciliation report of the Full audit (`write_reconciliation_report` in
  `full.rs`), covered by a caller-level swap test.

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
- Reads by path. `resolve_contained_read` canonicalizes, and a swap after it can be
  read through. The read-only views of `td_index` (`read_sheets`, `read_sheets_in`,
  `debt_entries`, `docs_index_rows`: cards, reviews, the cross-audit), the
  `td_decisions::snapshot` taken BEFORE the agent runs, `adr::read_decisions`, the
  `td_sheets` listing of `adr`, and the reconciliation snapshot reads of the Full
  audit are by path: nothing is written from them. Whatever is written from a
  listing or a read goes through the primitive (the resync, `restore`, the ADR
  settlement and the report), where a refusal is an error or a reported skip.
- Files a spawned `git` or an agent writes by path. The `git mv` of `docs_migration`
  is one (its paths are checked first and its fallback is rooted). So is the
  `git checkout <head> -- <file>` of `step_remit::restore_from_head`, which puts a
  tracked file of another AI-doc folder back: the bytes it replaces are read through
  the primitive first (a linked parent makes that read fail and no checkout runs),
  but git's own write is git's; git itself refuses to write beyond a symlink in the
  leading path.
- Other by-path writers that never call `fs_guard`: `root_agent_files::inject_or_update`,
  `skills`, `profiles`; in `full.rs` the audit-cancel cleanup (`remove_dir_all` of an
  empty docs folder after `classify_docs_dir_for_cancel`, and the removal of a leftover
  `ai` link); in `worktree` the copy of the untracked files into a new checkout and the
  submodule parent folders. `learning_doc.rs`, `context_files.rs` and `mcp_scanner.rs`
  also write by path; they are routes and settings writers, not run right after an
  agent, so they are outside this round's scope, not covered either.
- A `tech-debt/` folder that is itself a link: the resync and the ADR settlement
  refuse it (`guarded_list` / `guarded_read` error), `td_decisions::restore` aborts
  on it; the by-path read-only views treat it as empty. The containment of every
  write never depends on those readings.
- `template_reinstall` applies the template's file renames before it writes
  (`apply_renames_in`); a later failure rolls back the files it wrote, not those
  renames.

## Windows: residual risk, accepted

**On Windows the fix does not apply: the hole stays open.** There is no
directory-descriptor primitive in use on Windows. There, every `guarded_*` write
keeps the behaviour it had before KT-1595: the lstat walk, then the write by path.
A parent folder swapped for a link between the walk and the write is therefore
not excluded, for every caller listed above. Creating a symlink on Windows needs elevated
rights or developer mode, which is why this is accepted rather than failing
closed: worktree creation, template installs, native files and seeding must keep
working on the desktop build. The owner decided this and amended DoD item 3 of
KT-1595 accordingly (item 3 as written asked every target to fail closed).

The same holds for `guarded_list`, `guarded_read` and `guarded_create_new_dir` and for every third-pass caller above (the placeholder fill, the date stamp, `step_remit`, `anti_hallu_step`, the resync, `restore`, the git exclude, the backup folders and the report): on Windows each is the lstat walk followed by the operation by path, and the swap tests of those callers run on unix only.

Closing it needs handle-rooted file operations on Windows. That is the follow-up
ticket **KT-1609** (Windows rooted primitive), planned for 0.15.4; not done in 0.15.3.

Any target that is neither unix nor Windows fails closed: every `guarded_*` write
is refused and nothing is created (the `cfg(not(any(unix, windows)))` module in
`core/fs_guard.rs`).

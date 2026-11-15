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
  absent before the step). A file that was clean before the step is put back from
  the recorded commit WITHOUT `git checkout` (round 4): `git cat-file blob
  <head>:./<file>` gives the bytes, `git ls-tree` the recorded mode (`100644` or
  `100755`; a link or a submodule at that commit is refused and reported), and the
  bytes are written with `guarded_replace` (then `guarded_set_mode`). The blob is
  raw, with no smudge or end-of-line filter, unlike `git checkout`. The INDEX entry is
  put back too (round 5): after the `reset --soft` of a committed step, or an agent's
  `git add`, the index still holds the agent's version and the next commit would
  re-commit the reverted edit, so `git update-index --add --cacheinfo
  <mode>,<blob>,<prefix><file>` sets the entry from the recorded blob without touching
  the path (the index path is top-relative, hence the project's `--show-prefix`).
  A file that was already dirty keeps the user's whole index state too (rounds 7
  and 8): every `git ls-files -s` line (the three stages of a conflict included),
  the skip-worktree / assume-unchanged flag and an intent-to-add are recorded with
  its bytes, and the settle drops what the agent left (`update-index
  --force-remove`) and puts the record back (`--index-info`, `add -N`, the flag),
  then checks it; the entries go back NUL-separated (`update-index -z
  --index-info`), so a path holding a newline is one entry, never two. The whole
  index is read in four git calls (one snapshot, whatever the size of the
  repository), through `cmd::agent_touched_git_cmd` (rounds 10-11): hooks and
  fsmonitor off, every `filter.*` / `diff.*` / `merge.*` / `hook.*` entry of the
  EFFECTIVE configuration (includes and worktree config read too) turned off by
  its exact name, bytes included, through `GIT_CONFIG_COUNT` (a name that is not
  UTF-8 turns off that very driver, a name holding `=` stays one key; round 12),
  no replace refs, no lazy fetch, no optional index writes, no signature check,
  submodules ignored by status, the stat cache, `diff.relative` and `core.bare`
  pinned to git's defaults; every call runs under `cmd::run_bounded` (120 s, the
  whole process group killed on a timeout), so an include or a config file that
  is a FIFO fails the call instead of hanging. The configuration listing gets
  30 s, and a checkout where a git call ran out of time (the listing, or any
  call: an ignore file that is a FIFO blocks `check-ignore`) gets no git call for
  ten minutes: a blocking checkout costs one wait per pass, not one per call
  (rounds 12-13). Submodules are never looked into (`diff.ignoreSubmodules=all`):
  their own configuration is not in the listing (round 13). A configuration that cannot be read in time, or that declares
  more than 1 024 driver settings, runs nothing and is reported. The same
  hardened, bounded git (`cmd::agent_git_output`) serves the
  doc tooling that runs after an agent (round 12): the `checksums` listings,
  `doc_relevance`'s file listing and ignore check, the detectors' tracked listing
  and the `docs_citations` line map (`git diff --no-textconv --no-ext-diff`).
  The remit never reads a file past 64 MiB (`REMIT_CONTENT_MAX`) whole: such a
  file is left as it is and reported once it changed. A file that had a
  `filter` attribute (Git LFS) when the step started is never rewritten from its
  raw blob: with filters off its bytes always differ from the blob, so it is
  reported and left as it is; the attributes are read before the agent runs, so
  one it adds never spares its own edit (round 12). When the agent leaves a
  user's dirty file unreadable (a link, a FIFO, past 64 MiB), the user's version
  is kept as a blob the warning names, and what the user had staged comes back
  in every case; a refused entry has its index compared and put back too, and
  its fingerprint holds the change time, inode and device, so a same-size edit
  with its time put back is still seen (round 13). The `checksums` source-tree
  fingerprint hashes regular files only, in chunks, and reads a root AGENTS.md /
  CLAUDE.md only when it is (or links to) a regular file of at most 4 MiB: a
  FIFO or a device left by the agent never blocks it (round 13). The
  settle reports a git failure, a capture that could not be read (an unreadable
  HEAD in a repository makes a remit that only says so, never a silent unguarded
  step), more flagged files than are checked, and any change of the configuration
  or replace refs during the step. `git status` is parsed on bytes, a rename's
  origin always consumed. A path that is not
  UTF-8 is never written back. Not covered, by owner decision (KT-1769): an
  agent can still write `.git/config`, `.git/info` and `refs/replace` (the root
  fix is a sandbox write deny on `.git`); the change is reported, not undone, and
  a FIFO include still hangs Kronn's other git calls on that project, which are
  not bounded (`project_profile`, the citation merge-base). A file the user
  hid with one of those flags counts as dirty (its bytes are kept up to 2 000
  flagged files, its index state always), and a flag set or changed during the
  step is a change, so an edit hidden behind `--assume-unchanged` is restored or
  reported. A refused path (a link) the agent replaced is reported, and a file the
  agent added in another AI-doc folder too. The settle looks at every recorded
  file, not only at what `git status` lists, so a file the agent brought back to
  HEAD (`git restore`) gets the user's version back.
  A file whose bytes are back but whose index the agent changed is reported; a
  failed index restore is reported without hiding the content restore and its
  blob; a source file the agent only re-staged or flagged is listed with the
  other changed files, never touched. A
  failure of that command is reported as a warning. An AI-doc folder
  swapped for a link, or a link planted at the tracked name, is refused and reported
  as a warning ("could not be restored"); nothing is written or deleted through it.
  A file that could not be read at capture (a link) is never restored.
- `step_guard` (KT-1211: `restore_target`, `mark_partial`, `clear_partial_marker`, the
  keep-aside of `settle_killed`), run after a failed, cancelled or killed step: the
  target is read with `guarded_read` (round 4) after the lstat walk, because the
  content read is written back (the marker banner, the clean copy, the displaced copy).
  A link at the target or in a parent is an error (`refused`), never a copy of what it
  points at.
- `cross_audit::write_outputs` (step 8 of the cross-agent audit): the previous index,
  every existing `TD-<id>.md` (merge of the provenance block), the unchanged check of
  `write_if_changed` and `carried_from_previous` (the rows and settled sheets carried
  into the new index) all read with `guarded_read`. A link is an error and nothing is
  written from it. `core::cross_audit::assign_td_ids` still lists the sheets by path
  (see "What is not covered").
- `document_repair::demote_last_attempt` and `document_repair::snapshot`: the document
  is read with `guarded_read` before it is demoted, preserved in `.kronn/audit` or
  written back by an undo.
- `doc_provenance` (round 4): `read_doc` (every confirm, record and comparison),
  `read_strict` (the record itself) and `snapshot_tracked` (listed and read with
  `guarded_list` / `guarded_read`, no walk of a linked root) go through the primitive.
  A doc that is a link, or sits behind one, reads as ABSENT: it is never recorded as
  validated, never hashed, and never copied by `preserve_validated_original` into
  `.kronn/audit/validated-originals`. `snapshot_tracked` and `read_strict` return an
  error on a link.
- The reconciliation of the Full audit (`full::build_reconciliation`,
  `reconciliation::check_signature_in_source`, round 4): the indices written by the
  run, the pre-audit sheets, each sheet being checked and the source file a pointer
  names are read with `guarded_read`. A link at an index or a sheet stops the
  reconciliation (logged, the audit carries on); a pointer that is a link or leaves
  the project counts as unreadable.
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
  since the read is left alone and reported. The listing is `guarded_list`. A sheet that
  is not UTF-8 is skipped and reported (`refused`) like a link, and the duplicate-flagging
  pass (round 4) lists and reads through the primitive as well: a refused LISTING aborts
  the call (an error), a link at one sheet's name is skipped and reported.
- `adr::settle_covered_sheets`: no check by path before the read of `decisions.md`;
  the guarded read decides (missing is no decision, a link or an escape is an error).
  The sheet listing is `adr::td_sheets_guarded` (round 4, `guarded_list`): a link at
  `tech-debt/` or in a parent is an error, no longer "no sheets"; a sheet that is a
  link, or unreadable, is an error too; a folder or a non-UTF-8 sheet is skipped.
- `worktree::ensure_local_git_exclude_pinned` (round 4; reached from
  `audit_state::state_dir` on every state write, and from `workflows::workspace::
  work_to_keep`): git's own answer (`rev-parse --git-path info/exclude`) follows
  `.git/commondir` and a `.git` file's `gitdir:`, both writable by an agent, so the
  pinned variant does not use it. It writes `<repo>/.git/info/exclude` only, rooted at
  the REPO (a link at `.git`, `info` or `exclude`, planted or swapped in, is refused),
  and only when `.git` is a plain directory with no `commondir`. A `.git` file (a
  linked worktree or a submodule), a link, or a `commondir` is an error and nothing is
  written; the exclude of a linked worktree is the main repository's, written when the
  worktree was provisioned. The callers that concern a LINKED worktree after an agent
  (round 5, below) use `ensure_local_git_exclude_pinned_for(judge, registered_checkout,
  ...)`: the git directory is resolved from the REGISTERED checkout (`rev-parse
  --git-common-dir --show-prefix` run there, never in the lane), so the rule goes to the
  `info/exclude` of its common git directory, the file a linked worktree shares, even
  when the registered checkout is itself a linked worktree or the project sits below the
  repository top (the rule is then anchored under the project prefix, round 7). The
  write is rooted at that directory and a common directory that is a link is refused.
  The registered checkout is trusted there: it is not a workspace of the lane agent. The
  lane is only where git is asked whether the rule is
  already there, so a wrong answer skips a write, it never moves one. The write is create-only when the file is missing, an
  optimistic replace by rename otherwise (no longer an append: a line another process
  appends in that instant is refused, not lost silently). The original
  `ensure_local_git_exclude`, which does follow git's answer (rooted at the git
  directory git names, same refusals below it), stays for the callers that run before an
  agent works in the checkout: the `.kronn/` rule of `worktree` creation and the MCP
  settings route (`mcp_scanner`, not run after an agent). A residual: an agent that
  rewrote `commondir` or `gitdir:` of a repository before such a call would be followed.
- Lane provisioning and seeding (round 5, `core::task_worker_env`,
  `core::task_worker_lane_seeds`): both run AFTER an agent (the validations re-seed in
  `validate_and_apply`, a resume seeds again, and every worker launch provisions,
  correction rounds included), and a worker can rewrite `<lane>/.git` to
  `gitdir: /other/repo/.git`. The lane's git directory is therefore found by
  `worktree::registered_worktree_git_dir(main_checkout, worktree)`: from the registered
  checkout's own record (`<main>/.git/worktrees/<id>/gitdir` names the lane's `.git`
  file, read with `guarded_list` / `guarded_read` from the checkout), never from the
  lane's pointer. Git's answer (`rev-parse --absolute-git-dir`) is still asked for and a
  mismatch refuses everything (every seed is skipped with that reason). The seed marker
  is read with `guarded_read` and written with `guarded_replace`, rooted at the checkout,
  so a link planted at the marker, or the admin folder swapped for a link, is never
  written through. The exclude rules (`/.kronn-seeding-*`, `/.kronn-seeded-old-*`, the
  lane `target` rule, the `node_modules` link) go through the pinned variants above; a
  project whose registered checkout is itself a linked worktree, or sits below the repo
  top, gets its rule in the common git directory under its prefix (round 7). An agent can still write its OWN admin folder
  (for instance its `gitdir` back-pointer): that makes the lane unregistered, which is a
  refusal, never a fallback to git's answer.
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
  read through. These reads stay by path: the read-only views of `td_index`
  (`read_sheets`, `read_sheets_in`, `debt_entries`, `docs_index_rows`: cards, reviews),
  the `td_decisions::snapshot` and `adr::capture_records` / `adr::td_sheets` taken
  BEFORE the agent runs, `cross_audit::assign_td_ids` (it lists
  the sheets by path to choose or reuse an id: a sheet's text is not copied anywhere,
  but a swapped folder can change which id is chosen), `step_guard`'s
  `target_snapshot` (a comparison with the pre-step state) and `citation_drift::
  doc_files` (a listing that skips links), the TD counters of `full.rs`
  (`count_td_severities`, `compute_reconciliation_counts`,
  `compute_cluster_recommendations`: they only count, and take the sheet folder without
  the project root), the size heuristic of the step validation (`std::fs::metadata` of
  the target: a size) and the reads of Kronn's own templates. None of them writes what
  it read, and none puts more than a number or a boolean in a message; the claim of this
  document is only about the
  families listed under "Callers", not that no read in the code base is by path.
- Files a spawned `git` or an agent writes by path. The `git mv` of `docs_migration`
  is one (its paths are checked first and its fallback is rooted). The put-back of a
  file from HEAD in `step_remit` is no longer one (round 4: bytes from `cat-file`,
  written through the primitive).
- Other by-path writers that never call `fs_guard`: `root_agent_files::inject_or_update`,
  `skills`, `profiles`; in `full.rs` the audit-cancel cleanup (`remove_dir_all` of an
  empty docs folder after `classify_docs_dir_for_cancel`, and the removal of a leftover
  `ai` link); in `worktree` the copy of the untracked files into a new checkout and the
  submodule parent folders. `learning_doc.rs`, `context_files.rs` and `mcp_scanner.rs`
  also write by path; they are routes and settings writers, not run right after an
  agent, so they are outside this round's scope, not covered either.
- A `tech-debt/` folder that is itself a link: the resync, the ADR settlement and
  `td_decisions::restore` refuse it (`guarded_list` / `guarded_read` error); the by-path
  read-only views listed above treat it as empty. The containment of every write never
  depends on those readings.
- The pre-agent callers of `ensure_local_git_exclude` (see its entry above) follow git's
  answer; they never run after an agent.
- Round 5 moved to the primitive the read-only oracles of the post-agent code that were
  by path: `document_repair` (`content_hash`, `Blame::load`, `changed_recovery_paths`,
  the strict multi-citation read), `document_optimization` (`read` and `line_count`,
  rooted at the project), `validation` (`target_snapshot`, `unfilled_companion`, the
  leak check and the step-8 surface with its sheet listing), `adr::read_decisions`
  (rooted at the project since round 7, so a folder above a nested docs root swapped
  for a link reads as empty) and the quote read of `cross_audit::verify_citation`. A
  link there reads as absent or is an error, never as the content it points at.
  Rounds 7 to 9 closed the rest of `document_optimization`: a citation through a
  link (at the file or at a folder on the way, whatever it points at, a secret
  file included) or with a `.`/`..` component is refused up front with its own
  message and no stat of the target, a folder citation included; the binary sniff
  reads at most 8 KiB through the primitive (`guarded_read_head`), and a text file
  at most the verifier's 2 MiB cap (past it the bounds are not checked, as the
  verifier does);
  a file that cannot be read is never called empty (no `(empty)` rewrite from it);
  and an adapter reached through a link is skipped, never read. `adr`'s
  start-of-run capture of the `Decision record` lines is rooted at the project too.
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

The same holds for `guarded_list`, `guarded_read` and `guarded_create_new_dir` and for every third-pass caller above (the placeholder fill, the date stamp, `step_remit`, `anti_hallu_step`, the resync, `restore`, the git exclude, the lane seed marker, the backup folders and the report): on Windows each is the lstat walk followed by the operation by path, and the swap tests of those callers run on unix only.

Closing it needs handle-rooted file operations on Windows. That is the follow-up
ticket **KT-1609** (Windows rooted primitive), planned for 0.15.4; not done in 0.15.3.

Any target that is neither unix nor Windows fails closed: every `guarded_*` write
is refused and nothing is created (the `cfg(not(any(unix, windows)))` module in
`core/fs_guard.rs`).

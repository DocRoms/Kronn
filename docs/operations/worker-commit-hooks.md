# Git on a worker's worktree: hooks and pinned repository (KT-1745)

Security and worker-lane policy. Kronn inspects, stages, commits and merges for a
task worker in its own process tree: outside the worker's OS sandbox, as the host
user, with the user's credentials. Whatever git reads from the worktree and runs
there is therefore worker-chosen code running unconfined.

## What the worker controls

The worker writes its whole worktree, **including the `.git` pointer file in it**
and every file git reads from there:

- hooks, when `core.hooksPath` is relative (husky: `.husky`, `.husky/_`) — they
  resolve inside the worktree;
- `.gitattributes`, naming `filter=`, `merge=`, `diff=` drivers;
- through a redirected `.git` pointer (`gitdir: <a directory it wrote>`), a whole
  repository configuration: `filter.*.clean|smudge|process`,
  `merge.*.driver`, `diff.*.textconv`, `diff.external`, `core.fsmonitor`,
  `commit.gpgsign` + `gpg.program`, `core.hooksPath`, `http.*`, `url.*.insteadOf`.

A plain `git` run in that directory by Kronn follows the pointer and runs all of
it as the host user. The end-to-end proofs are in
`backend/src/core/lane_git_test.rs` (each with a control showing the plain git
does run the program).

## The rule: one builder, pinned to the registration

Every git command Kronn runs on a worker's worktree is built by
[src: file: backend/src/core/lane_git.rs:790-792#git]
([src: file: backend/src/core/lane_git.rs:63#LaneGit]), never by `git_cmd()`:

- **The repository is pinned, not discovered.** The command carries
  `--git-dir=<repo>/.git/worktrees/<lane>` and `--work-tree=<worktree>`. `<repo>`
  is the project checkout the worktree path sits under
  (`<repo>/.kronn/worktrees/<lane>`), its git directory is read from that
  checkout, and the `<lane>` entry is the one git itself registered for this
  worktree (`<admin>/gitdir`)
  ([src: file: backend/src/core/lane_git.rs:138-140#registered_admin_dir]). Neither
  the worktree's `.git` file nor a `commondir` is consulted.
- **The pointer is verified before every command**
  ([src: file: backend/src/core/lane_git.rs:661-663#verify]). If `<worktree>/.git`
  no longer reads `gitdir: <that admin dir>` (absolute or Kronn's portable
  relative form), nothing runs and the operation fails with
  `WorktreeTampered: …` — the text `task_exec_commit` returns to the worker, and
  the error the candidate build holds the integration with.
- **Git is given nothing to run**: `core.hooksPath=/dev/null`,
  `core.fsmonitor=false`, `commit.gpgSign=false`, `tag.gpgSign=false`
  (`--no-gpg-sign` on every commit), no `GIT_EXTERNAL_DIFF`, and diffs run with
  `--no-ext-diff --no-textconv`. With the repository pinned, a filter, merge
  driver or textconv program can only be one the *registered* repository's own
  configuration names (git-lfs, say), which the worker cannot write; an unknown
  `filter=` in a worktree `.gitattributes` resolves to nothing.
- A path that is not a managed worktree is the owner's own checkout: it keeps
  git's discovery and gets the same switches. A **push** keeps the owner's hooks
  there (`pre-push`)
  ([src: file: backend/src/core/lane_git.rs:835-838#git_keeping_owner_hooks]); from a
  managed worktree it is pinned and hook-less like the rest
  ([src: file: backend/src/api/git_ops.rs:1764-1767#run_git_push]), and the token-bearing
  authenticated push therefore reads the registered repository's configuration
  only ([src: file: backend/src/api/git_ops.rs:1727-1731#authenticated_push_command]).
  A task worker never pushes (only a human does, from the UI).

Call sites: the commit/merge commit, staging and identity of `task_exec_commit` /
`git_commit` ([src: file: backend/src/api/git_ops.rs:1316-1325#run_commit_command]),
the candidate merge, its abort and its commits
([src: file: backend/src/core/worktree.rs:2537-2547#build_candidate_with_hooks]), the doc
citations settling (`git checkout`, `add`, `grep`, `show`, `diff`), the worktree
inspections (`worktree_dirty_files`, `committed_file_changes`, `git_head`, …), the
HTTP agent's `git_status` / `git_diff` / `git_log`, the sub-task diff, the
time-limit checkpoint, the release and review checkouts' evidence patches and
status, the workflow Gate checkpoint and the sub-workflow engine commits (workflow
worktrees are agent-written too). A test fails when any of those functions calls a
plain `git_cmd()` ([src: file: backend/src/core/lane_git_guard_test.rs:76#plain_git]),
and a second one is **deny-by-default**: it scans every production file and fails
when a plain git spawn (`git_cmd()`, `async_git_cmd()`, `project_git_cmd()`,
`Command::new("git")`) appears that is neither marked `lane-git-allow: <why>` nor
within the per-file ceiling of `PLAIN_GIT_ALLOWED`, which only lists files whose
commands run in the registered project checkout, the owner's checkout or a
repository Kronn creates. The ceiling is a ratchet: a file whose count goes down
(a command moved to the lane builder) must lower its entry, so a later plain
command cannot hide in the slack.

Also pinned, because a worker's worktree can be their argument:

- a directory *inside* a managed worktree (`<wt>/pkg/deep`) resolves to the
  worktree's pin, so a subdirectory is never a way back to git's discovery;
- `resolve_commit` and `is_ancestor` (the first git call of the Git panel, of
  `run_integration`, of the send-back and of the checkpoint, handed a worker's
  worktree) and the `rev-parse --show-toplevel` of the submodule re-provisioning
  go through `git_keeping_owner_hooks`: in a managed worktree they are pinned (a
  redirected pointer is refused before git reads it), in the owner's checkout plain
  git. Every lane command also sets `GIT_NO_LAZY_FETCH=1`, so a promisor remote
  the worker wrote can never make a read fetch with its transport program
  ([src: file: backend/src/core/worktree.rs:2317-2322#resolve_commit]). The ratchet of
  the guard counts DIRECT spawns only: a plain helper handed a worktree path is
  not seen, so such a helper belongs in the function list of the guard test;
- `gh` and `glab` (`host_cli_command`: `gh pr create` runs `git status` itself)
  get the pin through the environment — `GIT_DIR`, `GIT_WORK_TREE`,
  `GIT_CONFIG_*` for hooks off, no file-system monitor — and, in a worktree whose
  pointer was redirected, a `GIT_DIR` that does not exist, so they run nothing
  ([src: file: backend/src/core/lane_git.rs:796-802#pin_foreign_command]);
- the Git panel of a workspace Kronn refuses to run git in says why (an
  unregistered directory under `.kronn/worktrees`, a redirected pointer, a
  submodule whose configuration was rewritten) instead of showing an empty answer
  ([src: file: backend/src/api/disc_git.rs:305#git_allowed_in]);
- the read helpers of the Git panel and the discussion routes (`run_git_status`,
  `run_git_diff`, `run_git_blame`, `run_git_commit_patch`, the commit and range
  pages, `disc_pr_template`, `disc_workspace`'s `git_value`) use
  `lane_git::git_or_refuse` / `git_keeping_owner_hooks`: in a managed worktree they
  are pinned and hook-less, in the owner's checkout they are plain git. When the
  worktree is refused (tampered pointer, nested repository), `git_or_refuse`
  returns a command aimed at a git directory that does not exist, so a helper
  that cannot carry an error answers "nothing" and runs nothing;
- a commit the *user* asks for from the UI (`run_git_commit`, hooks on) in a
  Kronn-managed worktree (a discussion's or an agent's) is the pinned, hook-less
  command as well: that worktree is agent-written, so its hooks are not the
  user's. Their own checkout keeps its hooks;
- `fast_forward_to` and the checks of `integration_target_worktree`: when the
  integration target is itself checked out in a Kronn-managed worktree, the
  fast-forward is the pinned, hook-less command; in the owner's checkout their
  hooks (`post-merge`) keep running;
- `ensure_local_git_exclude` (lane seeds, the lane's Cargo target) resolves the
  exclude file through the pinned command, refuses a symlinked `info` directory,
  and, for a managed worktree, refuses one that leaves the registered git
  directory.

The unconfined `main_checkout` mode of the setting no longer exists (see below),
so no hook of the owner's runs as the host user with a worker's tree as its
working directory.

## Submodules and nested repositories

A submodule is a repository of its own whose configuration the worker can write
when its `.git` is a directory inside the worktree (Kronn clones them that way).
`status`, `diff`, `add -A`, `add -u`, `commit -a` and `diff-index` recurse into it
and would run a `filter.*.clean` it names, as the host. Three layers:

- the builder sets `diff.ignoreSubmodules=dirty`, `submodule.recurse=false`,
  `status.submoduleSummary=false` and `diff.submodule=short` (a host
  `diff.submodule=diff` would start a diff inside the submodule). Kronn's own
  `status`, `diff` and `diff-index` also carry `--ignore-submodules=dirty` ON THE
  COMMAND LINE inside a managed worktree, which beats a worker-written
  `.gitmodules` `ignore = none`
  ([src: file: backend/src/core/lane_git.rs:869-874#ignore_submodules]): they still show a
  moved gitlink but never run `git status` inside the submodule;
- a gitlink is staged by its sha (`update-index --cacheinfo 160000,<sha>,<path>`,
  the sha read through the submodule's own pinned lane), never with `git add`, which
  would compare the submodule's working tree
  ([src: file: backend/src/api/git_ops.rs:1175#stage_gitlink_by_sha]);
- a directory inside a materialised submodule (`<wt>/lib`, `<wt>/lib/pkg`) is the
  SUBMODULE's own pinned repository (`--git-dir=<its git directory>`,
  `--work-tree=<it>`), vouched for by the enclosing lane's registration and the check
  below; it is not pinned to the parent, so `HEAD` there is the submodule's (what
  re-provisioning compares with its pin). A repository that is not a gitlink of the
  enclosing lane's index is one the worker made: Kronn runs no git command in it.
  Levels are vetted one by one (a submodule of a submodule is accepted when it is a
  gitlink of the already vetted submodule; at most 6 levels). A LINKED WORKTREE of the
  same repository inside the worktree (a PR checkout under `<wt>/.kronn/`) is not a
  submodule: git registered it, so it is pinned to its own registration and its own
  pointer is verified like any managed worktree's; a directory the worker made is not
  registered and stays refused
  ([src: file: backend/src/core/lane_git.rs:621#child]);
- before every command the builder lists the index's gitlinks (from the worktree's
  root, whatever directory the command starts in) and refuses
  (`NestedRepoUnsafe`) while one carries something it does not trust
  ([src: file: backend/src/core/lane_git.rs:539#check_nested_repos],
  [src: file: backend/src/core/lane_git.rs:451#vet_nested_dot_git]): a nested
  `.git/config` with any key outside `core.*` plumbing, `extensions.objectformat` /
  `extensions.refstorage` (a reftable clone), `remote.*.url|fetch`,
  `branch.*.remote|merge`, `user.*` (so no `filter.*`, `core.fsmonitor`,
  `include.*`, `gpg.*`); a config that is unreadable or not valid UTF-8 (git can
  parse bytes Kronn cannot read as text, so it fails closed); a `.git/commondir` or
  `.git/config.worktree` (git would read a configuration Kronn does not vet); or a
  `.git` file that does not point into the registered repository's own git
  directory;
- Kronn never stages a NEW embedded repository a worker created (it would become
  a gitlink whose configuration the worker wrote)
  ([src: file: backend/src/api/git_ops.rs:1255#refuse_new_embedded_repository]).

And the worker cannot write a nested `.git` in the first place, see below.

## Relative programs in the registered configuration

With the repository pinned, a `filter.*`, `merge.*.driver` or `diff.*.textconv`
program can only be one the registered (owner's) configuration names. But a
RELATIVE one (`filter.rel.clean = sh tools/clean.sh`) resolves in the worktree, so
it runs whatever the worker wrote there. When the builder makes a command it reads
those values and, for each that names worktree content (a relative path, or an
argument that is a file the worktree holds), passes
`-c filter.<x>.clean= -c filter.<x>.smudge= -c filter.<x>.process= -c filter.<x>.required=false`
(`merge.<x>.driver=false`, `diff.<x>.textconv=`/`command=` likewise), and logs it
once per key
([src: file: backend/src/core/lane_git.rs:240-244#neutralised_programs]). An absolute
program (`/usr/bin/git-lfs`) or a bare command name (`git-lfs clean -- %f`) is the
owner's own and is left alone.

## The worker cannot write its pointer

The pointer is the one file the pinned commands trust, so it is read-only to the
worker wherever it writes, and so is the `.git` of every nested repository that
EXISTS when the sandbox starts (a submodule; any case). A repository the worker
creates LATER (`git init vendor/pkg`, terraform git modules, Composer source
installs, Bundler git gems, SwiftPM checkouts) is not denied on any platform: the
nested-repository check above and the refusal to stage a new embedded repository
cover it.

- Kronn's Seatbelt profile denies writes to the pointer and, by a case-insensitive
  regular expression per directory, to the `.git` of the worktree and of each
  nested repository found at launch
  ([src: file: backend/src/core/task_worker_sandbox.rs:518#seatbelt_profile_read_only]);
  bubblewrap binds the pointer and every nested `.git` that exists at launch
  read-only
  ([src: file: backend/src/core/task_worker_sandbox.rs:598-601#bubblewrap_args_read_only]);
- Claude's task-worker settings carry `sandbox.filesystem.denyWrite` for the
  pointer and for each existing nested `.git` (and its content), and
  `Edit/Write/MultiEdit/NotebookEdit` permission denies
  ([src: file: backend/src/agents/runner.rs:13219-13229#claude_task_worker_settings]);
- the HTTP agents' write tools refuse any path with a `.git` component
  (compared without regard to case, explicitly) and a symlink to one
  ([src: file: backend/src/api/agent_workspace_tools.rs:434#resolve_writable_in_workspace]);
  a generic `Custom` HTTP connection is refused as a task worker altogether;
- Codex already keeps `.git` read-only.

This is defence in depth: the verification above is what makes a write that gets
through (a CLI whose sandbox Kronn does not control) harmless.

## The setting: `worker_commit_hooks`

Per project, in the project workspace (`PUT /api/projects/{id}/workspace`, field
`worker_commit_hooks`; a human route, not reachable by a worker's bridge token).
Default `off`. It decides which **gating hook** Kronn itself runs before its own
commit (`pre-commit`, or `pre-merge-commit` for a merge); git never runs a hook
for these commands, whatever the value.

| Value | What runs for a worker's commit / merge commit |
|---|---|
| `off` (default) | No hook. |
| `sandboxed` | The gating hook the repository configures, resolved against the worktree (a worker-written `.husky` included), **inside the lane's OS sandbox profile** (reads the filesystem, writes the lane's roots but not any `.git` entry, no network). A host with no usable sandbox refuses the commit instead of running it unconfined. The pointer is verified again before Kronn's own commit. |

There is **no unconfined mode**. A `main_checkout` value this setting briefly had
(the owner's hook run as the host user) is gone: a real hook executes worktree
content (`PATH=node_modules/.bin:$PATH`, lint-staged, `make`, pre-commit's local
repos), so even the owner's own file ran worker-reachable binaries as the host.
A stored `main_checkout` is read as `sandboxed`
([src: file: backend/src/models/projects.rs:37#WorkerCommitHooks]): a configured
hook runs confined or not at all.

The confined hook is bounded by the lane's 10-minute probe limit; a hook still
running then is killed and refuses the commit. Both of its streams are kept: a
small wrapper (`sh -c`) collects stdout and stderr in order, bounded to the last
8 KiB, and hands them back with their line breaks, so a linter that prints its
errors on stdout is not lost
([src: file: backend/src/core/commit_hooks.rs:214#HOOK_WRAPPER]).

**Not run under any value**: `commit-msg` (commitlint), `prepare-commit-msg`,
`post-commit`, `post-merge`, `reference-transaction`… Only the gating hook is
run, by Kronn. A repository that relies on a `commit-msg` check for a rule other
than DCO does not get it on a worker's commits.

The DCO check needs no `commit-msg` hook: Kronn signs every commit off itself
(`git commit -s`), so the sign-off is enforced whatever the policy.

The policy type is
[src: file: backend/src/models/projects.rs:37#WorkerCommitHooks].

## A refusal is its own outcome

A gating hook that exits non-zero is a `HookRefused`, with the first lines of its
output (20 lines / 2 000 characters), never a conflict
([src: file: backend/src/core/commit_hooks.rs:174#run_gating_hook]):

- `task_exec_commit` / `git_commit` fail with `HookRefused (pre-commit): <output>`
  and commit nothing;
- the candidate build returns `CandidateOutcome::HookRefused { hook, output }`
  ([src: file: backend/src/core/worktree.rs:2450#HookRefused]), undoes the merge
  and is reported to the principal and worker with the hook's output.

A `git merge` refused before it starts (a dirty tree, unrelated histories) is an
error carrying git's own words (`… was refused before it started: …`), which
holds the integration for the child worktree; it is never an empty
`Conflict { files: [] }`. A merge commit git itself rejects is an error too.

## Tests

- `backend/src/core/lane_git_test.rs`: the pointer is pinned to the registration
  (absolute and portable-relative forms); a redirected pointer (a repository whose
  configuration names a filter, a file-system monitor, a signing program, a merge
  driver, a hooks directory) makes every Kronn git command refuse with
  `WorktreeTampered` and none of those programs ever runs, with a control showing
  plain git does run the filter; a pointer into `<repo>/target/wt-<lane>` cannot
  pass for the main checkout under `sandboxed`; the owner's own hook never runs as
  the host user (there is no `main_checkout` mode); a submodule whose config the
  worker wrote is refused and its filter never runs (control: plain git runs it);
  a relative `filter.*.clean` of the registered config is turned off; a new
  embedded repository is never staged; a nested `.GIT` is refused to the HTTP
  write tools; a conflict abort fires no
  `reference-transaction`; a merge refused before it starts is an error; the HTTP
  write tools refuse the pointer.
- `backend/src/core/docs_citations/merge_tests.rs`: restoring staged docs fires no
  `post-checkout`.
- `backend/src/core/lane_git_test.rs` (round 4): `resolve_commit` / `is_ancestor` on
  a worktree redirected to a partial clone whose `ext::` transport is a worker
  program run nothing (control: plain `rev-parse` fires it); a materialised
  submodule is its own repository, not the parent's (`verify_worktree_head` on it);
  a submodule check also runs from a subdirectory; a program that starts git itself
  is pinned; an unregistered directory gets a clear message; the owner's own
  `pre-commit` still runs for their own commit.
- `backend/src/core/commit_hooks_test.rs`: a relative-hooksPath hook writing a
  marker outside the worktree is never run by a worker commit nor a candidate
  merge; a failing hook is `HookRefused` with its output and commits nothing; DCO
  sign-off stays enforced; a stored `main_checkout` reads as `sandboxed`;
  `sandboxed` cannot write outside the worktree; the wrapper keeps both streams,
  their line breaks and the exit status.
- `backend/src/core/task_worker_sandbox_test.rs` (also: only existing nested
  repositories are denied, a confined command can create a nested repository) and
  `backend/src/agents/runner_test.rs`: the pointer is read-only in the profile
  (and, where the OS sandbox can be applied, to a real confined command) and in
  Claude's settings.
- `backend/src/core/lane_git_guard_test.rs`: no plain `git_cmd()` in a function
  that runs git on a worker's worktree.

## Residual risk

- A CLI worker's own sandbox is not Kronn's: if it lets the worker write the
  repository's *shared* git directory (`<repo>/.git`, where the registered
  configuration and `hooks/` live) the pinned commands trust what it wrote. Kronn's
  own profiles and Claude's task-worker settings write only the worktree and the
  lane's Cargo target.
- `commit-msg` hooks (commitlint) are not run for a worker's commit under any
  policy; only the DCO sign-off is enforced.
- A nested `.git` created after the sandbox started is not denied on any platform
  (so `git init vendor/pkg` works); Kronn's nested-repository check and the refusal
  to stage a new embedded repository cover it.
- `fast_forward_to` and the integration target's checks run in the target
  checkout with the owner's hooks, as the owner's own git does; the target is not a
  worker lane.
- Discussion worktrees outside the task lanes share the builder only where their
  code path goes through the functions listed above.

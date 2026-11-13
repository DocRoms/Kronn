# Git hooks of Kronn's commits and merges for a worker (KT-1745)

Security and worker-lane policy. Kronn commits and merges for a task worker in
its own process tree: outside the worker's OS sandbox, as the host user, with
the user's credentials. Letting git run the repository's hooks there runs
whatever those hooks point at.

## The hole this closes

A relative `core.hooksPath` (husky: `.husky`, `.husky/_`) resolves **inside the
worktree**, which the worker writes. Before this change a worker could edit
`.husky/pre-commit`, call `task_exec_commit` (or wait for the candidate merge)
and Kronn would execute that script unsandboxed. Functionally the same path
made every worker commit fail on a repository whose tracked hook needs a
gitignored file (`_/husky.sh`) the fresh worktree lacks, and the candidate merge
reported that failure as an empty conflict, so the worker looped.

## The rule

No Kronn-run git command of a worker lane lets git resolve a hook inside the
worktree. Each carries `-c core.hooksPath=<absolute dir outside the worktree, or
/dev/null>` and `-c core.fsmonitor=false`
([src: file: backend/src/core/commit_hooks.rs:134#git_config_args]). The two
Kronn-made commits are covered:

- `task_exec_commit` / `git_commit` — `run_commit_command`
  ([src: file: backend/src/api/git_ops.rs:1173#run_commit_command]);
- the candidate merge and the doc re-anchoring commit —
  `build_candidate_with_hooks`
  ([src: file: backend/src/core/worktree.rs:2520#build_candidate_with_hooks]).

A commit the user asks for in their own checkout (`POST /git/commit`) is
unchanged: the repository's hooks run, as ever.

The DCO check needs no `commit-msg` hook: Kronn signs every commit off itself
(`git commit -s`), so the sign-off is enforced whatever the policy.

## The setting: `worker_commit_hooks`

Per project, in the project workspace (`PUT /api/projects/{id}/workspace`,
field `worker_commit_hooks`; a human route, not reachable by a worker's bridge
token). Default `off`.

| Value | What runs for a worker's commit / merge commit |
|---|---|
| `off` (default) | No hook. |
| `main_checkout` | The gating hook (`pre-commit`, `pre-merge-commit` for a merge) from `<git-common-dir>/hooks` of the main checkout, as the host user. It is the owner's own hook, never a file of the worktree. When that directory lies inside the checkout being committed, nothing runs. |
| `sandboxed` | The gating hook the repository configures (a worktree `.husky` included), **inside the lane's OS sandbox profile** (read the filesystem, write the lane's roots only, no network). A host with no usable sandbox refuses the commit instead of running it unconfined. |

Whatever the value, git itself is given a hooks directory that is `/dev/null` or
the main checkout's, so no passive hook (`post-commit`, `post-merge`,
`prepare-commit-msg`…) of the worktree runs
([src: file: backend/src/core/commit_hooks.rs:119#hooks_path_for_git]).
The policy type is
[src: file: backend/src/models/projects.rs:37#WorkerCommitHooks].

## A refusal is its own outcome

A gating hook that exits non-zero is a `HookRefused`, with the first lines of its
output (20 lines / 2 000 characters), never a conflict
([src: file: backend/src/core/commit_hooks.rs:217#run_gating_hook]):

- `task_exec_commit` / `git_commit` fail with `HookRefused (pre-commit): <output>`
  and commit nothing;
- the candidate build returns `CandidateOutcome::HookRefused { hook, output }`
  ([src: file: backend/src/core/worktree.rs:2423#HookRefused]), undoes the merge
  and is reported to the principal and worker with the hook's output.

A `git merge` refused before it starts, or a merge commit git itself rejects, is
no longer an empty `Conflict { files: [] }` either: git's message is logged or
returned as the error.

## Tests

`backend/src/core/commit_hooks_test.rs`: a relative-hooksPath hook writing a
marker outside the worktree is never run by a worker commit nor a candidate
merge; a failing hook is `HookRefused` with its output and commits nothing; DCO
sign-off stays enforced; `main_checkout` runs the common-dir hook and not the
worktree's; `sandboxed` cannot write outside the worktree.

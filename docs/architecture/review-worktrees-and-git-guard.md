# Review worktrees and the main-checkout git guard (KT-1645)

2026-11-09: a Claude Code review discussion created through the API worked in the
owner's main checkout, which held uncommitted files. One of its subagents ran
`git checkout --detach`, then restored HEAD with `git symbolic-ref`. Nothing was
lost, but a reviewer could move HEAD or touch uncommitted work. Two measures close
it.

## A review gets a detached worktree of its own

`CreateDiscussionRequest.review_head` (and optionally `review_base`) marks a
discussion as a review of a commit range. `POST /api/discussions` then:

1. resolves both revisions to commits (an option-like or unknown revision is
   refused) and creates a **detached** worktree at the head under
   `.kronn/worktrees/<project>--review-<id>`;
2. stores the discussion with `workspace_mode = "Review"`, the worktree as
   `workspace_path` and no `worktree_branch` (nothing to switch back to);
3. on any refusal (no project, unknown revision, `git worktree add` failing) fails
   the creation and removes what it made. It **never** falls back to the project's
   checkout, whatever `workspace_mode` the request asked for.

Guards that keep it that way after creation:

- the agent start (`streaming.rs`, the preflight next to the scratch-directory one)
  refuses a `Review` discussion whose `workspace_path` is empty, instead of using
  the project path;
- `worktree_unlock` refuses a `Review` discussion (there is no branch to re-lock);
- deleting the discussion removes the worktree like any other discussion worktree.

[src: file: backend/src/core/worktree.rs#create_review_worktree]
[src: file: backend/src/api/discussions/crud.rs#provision_review_workspace]

## Agents cannot move a main checkout

Every CLI agent launch (`agents::runner::try_spawn`) gets a `git` shim first on its
`PATH`, written to `<work_dir>/.kronn/bin/git` (git-ignored; inside the working
directory because the Kronn data directory is closed to agents by
[agent-vault-protection.md](agent-vault-protection.md)). The shim refuses
`git checkout`, `git switch`, `git reset` and `git stash` (except `stash list` and
`stash show`) when the repository is a **main checkout** (its git dir is its common
git dir). The agent sees:

```
kronn: refused 'git checkout' in /path/to/project, the project's main checkout.
It holds the owner's current branch and uncommitted files, and an agent must not move them.
Work in a dedicated worktree instead (git worktree add <dir> <rev>), or ask the human to run the command or to confirm it.
Nothing was changed.
```

A linked worktree (an agent's own checkout, a task worker's lane, a review) and
every other git command go straight to the real git. `-C <dir>` and git's global
options are followed, so `git -C /main checkout` from a worktree is refused too.

The human's confirmation is `KRONN_MAIN_CHECKOUT_GIT_CONFIRMED=1` in the launch
environment. Nothing sets it yet: there is no confirmation UI, so today the human
runs the command themselves.

Limits: it is a net against accidents, not a sandbox. An absolute path to git
(`/usr/bin/git`) and git aliases are not seen, and Windows / WSL agents get no
shim. [src: file: backend/src/core/git_guard.sh]
[src: file: backend/src/core/git_guard.rs]

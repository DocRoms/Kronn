# Commit identity, sign-off and OS junk (KT-1750)

Kronn makes commits for a project (a worker's `task_exec_commit`, the Git panel,
the merge of the target branch into a task branch, the re-anchoring of doc
citations, the WIP checkpoint of a worker stopped by the time limit). Who they are
attributed to, and whether they carry a `Signed-off-by`, are **per-project
settings** with a detected default. Nothing is written into the repository's git
config, and no global identity is used silently.

**Owner rule: project and toolchain specifics are settings shown in the UI, with an
auto-detected default — never a hidden git key or an environment variable.**

## Where it is set

The project card (Dashboard › project › *Identité et signoff des commits*) saves
`ProjectCommitSettings` inside the project workspace
(`PUT /api/projects/{id}/workspace`, stored in `workspace_json` next to the
worktree hooks). [src: file: backend/src/models/projects.rs:55#ProjectCommitSettings]

- **Name and e-mail**: both or neither (the API refuses half an identity). Empty
  means *detected*.
- **Sign-off**: `auto` (default), `always`, `never`.

`GET /api/projects/{id}/commit-policy` returns what the card shows: the saved
settings, what the repository says by itself (`detected`), what Kronn commits with
(`effective`), where each answer comes from, and `global_identity`.
[src: file: backend/src/api/projects/commit_policy.rs:73#get]

## How the policy is resolved

`core::commit_policy` resolves the identity and the sign-off of the repository a
path belongs to (a linked worktree answers for its repository).
[src: file: backend/src/core/commit_policy.rs:126#for_repo]

| | Order |
| --- | --- |
| Identity | the card's name + e-mail → `kronn.commitName` + `kronn.commitEmail` of the repository's git config (a **detection source**, not the way to set it) → the repository's own `user.name` + `user.email` → the global git config (**used, but announced**) → the placeholder `Kronn <kronn@localhost>` |
| Sign-off | the card's *Always* / *Never* → `kronn.signoff` of the repository's git config → the briefing's `signoff_required` → a DCO file → a `commit-msg` hook that checks `Signed-off-by` → most of the last 30 non-merge commits signed → off |

The identity reaches git as `GIT_AUTHOR_*` / `GIT_COMMITTER_*`
(`CommitPolicy::apply_identity`); an amend keeps its author. The worker brief says
`git commit -s` only when the policy signs off.

The merge and worker-commit code has no database, so the card's settings are
published to the process by `register_project` — at boot (`main.rs`) and each time
a card saves them. [src: file: backend/src/core/commit_policy.rs:209#register_project]

## The global identity is never silent

When the identity can only come from the global git config (the repository has no
`user.email` and the card sets none), Kronn keeps committing — it never blocks —
but:

- the worker room and the principal room each get one notice the first time a
  worker commits (`orch-commit-identity:<execution>:worker|principal`, so a resume
  cannot post it twice): *Ce projet n'a pas d'identité de commit : Kronn utilise
  `<email>`. Règle-la dans la carte du projet.*
  [src: file: backend/src/api/orchestration.rs:12100#announce_global_commit_identity]
- the project card shows the same warning (`global_identity`).

## OS junk

`.DS_Store`, `Thumbs.db`, `desktop.ini` and `._*` are never a worker's work.
The list is **Settings › Workers › OS junk files** (`workers.os_junk_patterns`),
validated on save (`WorkersConfig::validate`, `os_junk_pattern_problem`) and
published to the process by `config::save` and at boot
(`worktree::set_os_junk_patterns`).
[src: file: backend/src/core/worktree.rs:1532#set_os_junk_patterns]

- Each lane Kronn creates excludes the patterns in the repository's local exclude
  (`ensure_os_junk_excluded`), without touching a tracked `.gitignore`.
  [src: file: backend/src/core/worktree.rs:1577#ensure_os_junk_excluded]
- A checkout that holds only junk is not dirty: delivery and cleanup go through.
  A *tracked* junk file that changed is still work.
  [src: file: backend/src/core/worktree.rs:4186#worktree_dirty_files]
- A type change (`T`, a file replaced by a symlink) is delivered as *Modified*.

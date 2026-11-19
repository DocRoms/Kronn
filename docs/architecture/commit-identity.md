# Commit identity, sign-off and OS junk (KT-1750)

Kronn makes commits for a project (a worker's `task_exec_commit`, the Git panel,
the merge of the target branch into a task branch, the re-anchoring of doc
citations, the WIP checkpoint of a worker stopped by the time limit). Who they are
attributed to, and whether they carry a `Signed-off-by`, are **settings** — one
global, one per project overriding it — with a detected default. Nothing is written
into the repository's git config, and no machine identity is used silently.

**Owner rule: project and toolchain specifics are settings shown in the UI, with an
auto-detected default — never a hidden git key or an environment variable.**

## Where it is set

Same model as the lane warm-up settings (KT-1742): a global setting, a per-project
override.

- **Global — Settings › Workers › *Commits Kronn*** (`workers.commit`, the same
  `ProjectCommitSettings` shape): name, e-mail and sign-off (Auto / Always / Never).
  Validated on save (`WorkersConfig::validate`) and published to the process by
  `config::save` and at boot (`commit_policy::set_global_settings`).
  [src: file: backend/src/models/setup.rs:170#commit]
  [src: file: backend/src/core/commit_policy.rs:221#set_global_settings]
- **Project — the card** (Dashboard › project › *Identité et signoff des commits*)
  saves `ProjectCommitSettings` inside the project workspace
  (`PUT /api/projects/{id}/workspace`, stored in `workspace_json` next to the
  worktree hooks). [src: file: backend/src/models/projects.rs:66-72#ProjectCommitSettings]
  Its sign-off defaults to *Hériter* (`auto`) and its name / e-mail fields to empty.
- **Name and e-mail**: both or neither (the API refuses half an identity). Empty
  means *inherit*.

`GET /api/projects/{id}/commit-policy` returns what the card shows: the saved
settings, what the project inherits (`detected`: repository → Settings › Workers →
machine), what Kronn commits with (`effective`), where each answer comes from, and
`global_identity`.
[src: file: backend/src/api/projects/commit_policy.rs:75#get]
When that call fails the card says so in the user's language, keeps the form
editable (the saved settings are read from the workspace) and disables only
*Utiliser les valeurs détectées*, with the reason as its title.

## How the policy is resolved

`core::commit_policy` resolves the identity and the sign-off of the repository a
path belongs to (a linked worktree answers for its repository).
[src: file: backend/src/core/commit_policy.rs:136-139#for_repo]

| | Order, first that decides |
| --- | --- |
| Identity | 1. the card's name + e-mail → 2. the repository's own: `kronn.commitName` + `kronn.commitEmail` of its git config (a **detection source**, not the way to set it), then its local `user.name` + `user.email`, then an identity git gives **this directory only** (a conditional include of the global config — `includeIf` on `gitdir:`, `hasconfig:`, `onbranch:` — or the worktree config; source `conditional`) → 3. the Settings › Workers identity → 4. the machine's global git config (**used, but announced**) → 5. the placeholder `Kronn <kronn@localhost>` |
| Sign-off | 1. the card's *Always* / *Never* → 2. the repository's own: `kronn.signoff` of its git config, the briefing's `signoff_required`, a DCO file, a `commit-msg` hook that checks `Signed-off-by`, or most of the last 30 non-merge commits signed (or unsigned) → 3. the Settings › Workers *Always* / *Never* → 4. off |

A company repository's own identity is therefore never overwritten by a personal
global one: the Settings › Workers identity only fills what the repository leaves
open (`choose_identity`, `inherit_signoff`). An effective `user.*` identity that
differs from both the plain `--global --no-includes` and `--system --no-includes`
files is directory-specific and ranks with the repository's own, so a personal
Kronn global identity never signs a company repository that git already
identifies (`resolve_identity`). [src: file: backend/src/core/commit_policy.rs:368-371#choose_identity]
[src: file: backend/src/core/commit_policy.rs:236-238#inherit_signoff]

The identity reaches git as `GIT_AUTHOR_*` / `GIT_COMMITTER_*`
(`CommitPolicy::apply_identity`); an amend keeps its author. The worker brief says
`git commit -s` only when the policy signs off.

The merge and worker-commit code has no database, so the card's settings are
published to the process by `register_project` — at boot (`main.rs`) and each time
a card saves them. [src: file: backend/src/core/commit_policy.rs:262-264#register_project]

## The machine identity is never silent

When the identity can only come from the machine's global git config (step 4: the
repository has no `user.email`, the card sets none and Settings › Workers sets
none), Kronn keeps committing — it never blocks —
but:

- the worker room and the principal room each get one notice the first time a
  worker commits (`orch-commit-identity:<execution>:worker|principal`, so a resume
  cannot post it twice): *Ce projet n'a pas d'identité de commit : Kronn utilise
  `<email>`. Règle-la dans la carte du projet.*
  [src: file: backend/src/api/orchestration.rs:12846-12850#announce_global_commit_identity]
- the project card shows the same warning (`global_identity`).

An identity set in Settings › Workers is a decision, so it raises no notice.

## OS junk

`.DS_Store`, `Thumbs.db`, `desktop.ini` and `._*` are never a worker's work.
The list is **Settings › Workers › OS junk files** (`workers.os_junk_patterns`),
validated on save (`WorkersConfig::validate`, `os_junk_pattern_problem`) and
published to the process by `config::save` and at boot
(`worktree::set_os_junk_patterns`).
[src: file: backend/src/core/worktree.rs:1830-1831#set_os_junk_patterns]

- Each lane Kronn creates excludes the patterns in the repository's local exclude
  (`ensure_os_junk_excluded`), without touching a tracked `.gitignore`.
  [src: file: backend/src/core/worktree.rs:1874-1876#ensure_os_junk_excluded]
- A checkout that holds only junk is not dirty: delivery and cleanup go through.
  A *tracked* junk file that changed is still work.
  [src: file: backend/src/core/worktree.rs:4730-4736#worktree_dirty_files]
- A type change (`T`, a file replaced by a symlink) is delivered as *Modified*.

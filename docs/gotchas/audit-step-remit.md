# Audit steps stay inside their remit (KT-1360)

An audit step writes documentation under the project's docs root and nothing
else. Two escapes were seen on the bench matrix, and the prompt alone does not
stop either, so a deterministic guard backs the prompt.

## What the guard does

`StepRemit` in `backend/src/api/audit/step_remit.rs` is captured before the
agent runs and settled after every attempt, in the Full pipeline
(`backend/src/api/audit/full.rs`) and the partial pipeline
(`backend/src/api/audit/drift.rs`).

- **Commits.** HEAD and the branch are recorded. If HEAD moved by plain
  (non-merge) commits that all sit on top of the recorded HEAD, on the same
  branch, authored at or after the step start, **and every commit changes only
  paths under the docs root, a Kronn-owned pointer or another AI-doc folder**
  (`git diff-tree` per commit), Kronn runs `git reset --soft <recorded HEAD>`:
  the changes stay in the index and the worktree. Any other move (a user commit,
  a commit touching source, an amend or rebase, a branch switch) is never
  touched, only reported, naming the foreign paths. New branches and tags are
  reported, not deleted.
- **Another AI-doc folder.** A folder outside the root is recognised before the
  step from the tracked files: it holds a `.kronn.json`, a Kronn-signed
  `AGENTS.md` (same check as `is_kronn_managed_docs`), an `inconsistencies-*.md`,
  or a `tech-debt/TD-*.md` (the folder is the parent of `tech-debt/`). A plain
  `AGENTS.md` of the user (say `src/AGENTS.md`) does not qualify.
- **Files outside the docs root.** Only a tracked file of another AI-doc folder
  that the step changed (or deleted) is restored: to the recorded commit when it
  was clean, to the captured bytes when the user already had uncommitted edits
  there. **Before any overwrite the current bytes are written as a git blob**
  (`git hash-object -w`) and the report line carries the sha
  (`git cat-file -p <sha>`); if the blob cannot be written, the file is not
  touched. Any other tracked change outside the root (source, config) is only
  reported in a warning naming the files, never touched. Untracked files are
  left alone. Kronn-owned paths (root pointers such as `CLAUDE.md`/`AGENTS.md`,
  per-agent config folders, `.mcp.json`, `.kronn/`) are exempt. When HEAD moved
  and was left as it is, the check is skipped (a stale commit is not a
  pre-step state).
- **Reporting.** Each finding is a `step_warning` SSE event, the `remit` array
  of `step_done`, and the step's persisted warning text. None of them changes
  the step's success.

## What the prompt says

`remit_prompt_block` is appended to every step prompt by `dated_step_prompt`
(`backend/src/api/audit/mod.rs`): write only under the root, another AI-doc
folder is read-only history (its TD sheets are not reused: update the ones
under the root), never run `git commit`/`push`/`tag`/`branch`/`checkout`/..., and
the repository's own commit conventions do not apply to an audit.

## Tool-level denial: possible for Claude Code, not wired

`claude --help` lists `--disallowedTools` (for example `"Bash(git *)"`), so a
deny rule is available for the direct Claude CLI route. Kronn does not pass it:
audits also run through the ACP adapter, HTTP agents and Codex (whose CLI offers
a sandbox mode and execpolicy `.rules` files, not a per-command deny flag), and
a prefix pattern is bypassed by `git -c k=v commit` or a `sh -c` wrapper. The
reset-based guard is the net that holds on every route; a deny rule would only
be an extra layer on one of them.

## Limits (what the guard cannot know)

An audit runs 30-60 minutes on the user's working repository and the user keeps
working in it. Kronn cannot tell who made a change, so:

- A user commit made during the step that touches **only** the root, a pointer
  or an AI-doc folder is indistinguishable from the agent's and is soft-reset
  (the changes stay staged; the report names the commit).
- A user edit made during the step inside another AI-doc folder is restored to
  the pre-step content; the edit survives only as the reported blob. Unreferenced
  blobs are pruned by `git gc` after its grace period (two weeks by default), so
  recover them promptly.
- An edit outside the root and the AI-doc folders is never reverted: if the
  agent made it, the warning is the only remedy and the user reverts it.
- The AI-doc folder set is read from tracked files at capture: a folder created
  by the step itself is not recognised.

Tests: `backend/src/api/audit/step_remit_tests.rs` (git fixture repositories).

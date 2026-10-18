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
  branch, authored at or after the step start, Kronn runs
  `git reset --soft <recorded HEAD>`: the changes stay in the index and the
  worktree. Any other move (a user commit, an amend or rebase, a branch switch)
  is never touched, only reported. New branches and tags are reported, not
  deleted.
- **Files outside the docs root.** Tracked files outside the root that the step
  changed (or deleted) go back to their pre-step content: the recorded commit
  when they were clean, the captured bytes when the user already had
  uncommitted edits there. Untracked files are left alone. Kronn-owned paths
  (root pointers such as `CLAUDE.md`/`AGENTS.md`, per-agent config folders,
  `.mcp.json`, `.kronn/`) are exempt. When HEAD moved and was left as it is, the
  restore is skipped (a stale commit is not a pre-step state).
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

Tests: `backend/src/api/audit/step_remit_tests.rs` (git fixture repositories).

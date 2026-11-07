# Claude Code refuses edits under `.cargo` (and other sensitive directories)

KT-1541. A task worker's `Edit` on `backend/.cargo/audit.toml` came back
`failed` with no reason, while `Cargo.toml`, lockfiles and `.rs` files in the
same worktree edited fine, and the principal edited the same file by hand.

## Which layer refused

Claude Code itself, not Kronn's settings and not the OS sandbox.

- Claude Code 2.1.285 keeps a built-in list of sensitive directories
  (`DANGEROUS_DIRECTORIES`: `.git`, `.vscode`, `.idea`, `.claude`, `.husky`,
  `.cargo`, `.devcontainer`, `.yarn`, `.mvn`). A write whose path has one of
  those as a component fails `checkPathSafetyForAutoEdit` and asks:
  `Claude requested permissions to edit <path> which is a sensitive file.`
- The ask comes **before** the `allow` rules and applies in `acceptEdits` mode
  (the worker's mode, `backend/src/agents/runner.rs`). A `--print` worker cannot
  answer it, so the call is refused.
- Kronn's own layers do not name `.cargo`: the vault deny list
  (`core::vault_guard::claude_permission_deny`) names the data directory only,
  and the worktree sandbox profile allows the whole worktree. The same ask also
  fires for a shell redirection into `.cargo`.
- The `user` line of the stream carries the refusal (an `is_error` tool result)
  and nothing else did, so Kronn's transcript kept no reason.

## What Kronn does now

- `claude_project_cargo_hooks` (`backend/src/agents/runner.rs`) adds a
  `PermissionRequest` hook to the worker's `--settings`. Deny rules are decided
  before the ask, so the Kronn data directory stays refused; the other
  sensitive directories keep asking, so keep being refused.
- The hook's `if` (`Edit|Write(//<worktree>/**)`) is only a pre-filter. The
  decision is the script `CLAUDE_CARGO_HOOK_SCRIPT`, which reads
  `tool_input.file_path` and prints `allow` only when the path is spelled
  without `.`/`..`/escape, sits under the canonical worktree, goes through a
  `.cargo` directory, has **no symlink among its components** (the file and
  every directory above it; a component that does not exist yet is fine, a new
  `.cargo/config.toml` is), has a nearest existing ancestor that canonicalizes
  under the worktree, and is not an existing file with another hard link. Any
  other case prints nothing: no decision, the ask stands, the call is refused.
  The walk up to the nearest existing ancestor stops after 64 steps, so a
  worktree that vanishes under the worker ends the hook at once instead of
  leaving it to Claude's hook timeout.
- A task worker's stream is read by `parse_claude_worker_stream_line`, which
  writes a refused tool call into the transcript as one line that starts with
  `> ⚠ Tool call refused: <reason>` (`REFUSED_TOOL_CALL_LINE_PREFIX`). It is used for task workers only: the refusal carries local
  absolute paths, and every other Claude run (workflow steps, summaries,
  ordinary discussions) keeps `parse_claude_stream_line`, which leaves it out. A
  result is a refusal only if it is an `is_error` tool result whose **first
  line** carries one of the CLI's permission wordings and it does not start with
  `Exit code` (command output that happens to say `Refusing to...` is not one).
- The reasons are read back from the worker's transcript
  (`api::discussions::runtime::refused_tool_calls_of_worker`, last three): only
  lines that start with that exact prefix count, so prose quoting the words (this
  page does) is not a refusal, and only the **current attempt** counts, that is
  the Agent messages after the room's last User message (the brief, then each
  round of the reviewer's findings). The "worker ended without a delivery" notice repeats them, and a worker that
  delivers anyway gets them as `refused:` attention points of its
  `delivery_summary/v1` (`ExecutionFacts::refused_tool_calls`).

## What the hook widens

A worker can now write the project's `.cargo/config.toml` (runner, `rustc-wrapper`,
aliases) and `.cargo/audit.toml`. Those run in later cargo builds, the human's
and CI's included. That is no wider than what a worker can already write: a
`build.rs` or a proc-macro also runs code in every later build. The hook does
not extend the reach beyond the worktree, which is why it refuses symlinks:
Edit and Write are not under the Bash sandbox, and a `wt/.cargo` pointing at
`~/.cargo` would otherwise have put a `rustc-wrapper` in the human's own
`~/.cargo/config.toml`.

## Caveats

- Claude's own symlink refusal only fires when the file itself is a link, not a
  directory above it. That is why the hook does the check itself rather than
  trusting it.
- The check and the write are two steps: a link created between them (another
  tool call running in parallel) is not caught by the hook. That window is not
  new to the hook: an ordinary in-worktree edit under `acceptEdits` is checked
  by Claude and then written the same way, so it shares it. Edit and Write run
  in the Claude process, **outside** the Bash sandbox; the sandbox confines
  commands only and does not cover them. What stays in force on this path is the
  data-directory deny rules.
- The refusal detector matches the CLI's English wording (read in 2.1.285); a
  reworded refusal would be silent again. After a Claude Code upgrade, re-read
  the sensitive-directory list as well: it may grow, and the hook only covers
  `.cargo`.

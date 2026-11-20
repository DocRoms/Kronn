# Kronn and the user's global git config (KT-1746)

Every git process of the machine reads `~/.gitconfig`. Older versions appended
a `safe.directory` entry on every workflow run, without the Docker guard and
without an existence check, and the test suite did the same with its temp
directories: 34 727 entries (3.1 MB) made `git rev-parse` ~76 ms instead of
~19 ms for every project and tool on the owner's Mac.

## Rules

- The only writer is `ensure_safe_directory` (and its async twin): Docker only,
  never an entry that is already listed (or `*`).
  [src: file: backend/src/core/git_global_config.rs:65]
- Do not call `git config --global` anywhere else in Kronn.
- Every test process points `GIT_CONFIG_GLOBAL` at a scratch file and
  `GIT_CONFIG_SYSTEM` at the null device, from a constructor that runs before
  `main` (lib test build and `tests/it.rs`). A new test binary needs the same
  constructor. [src: file: backend/src/core/git_global_config.rs:108]
  [src: file: backend/tests/it.rs:36]
- `ensure_safe_directory` refuses to write in a test process that was not
  isolated, so a harness that forgets it fails safe.
  [src: file: backend/src/core/git_global_config.rs:94]

## Repairing a file the old versions bloated

`kronn --clean-git-config` lists the temp-dir, vanished-worktree and duplicate
`safe.directory` entries, prints the entry count and `git rev-parse` time, and
removes them only after a backup (`<file>.kronn-backup-<epoch>`) and a `y`
answer. Git rewrites the file, so every other key is left alone.
[src: file: backend/src/core/git_global_config.rs:303]

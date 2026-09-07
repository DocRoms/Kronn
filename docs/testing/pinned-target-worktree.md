# Integration follows the pinned branch's checkout

An orchestration run pins a local target branch. Its Git repository may also
have a primary checkout on a different branch. Integration must locate the
target branch's registered worktree rather than run `git merge` in the primary
project directory.

`integration_target_worktree` in `backend/src/core/worktree.rs` requires exactly
one registered checkout. It reads the NUL-delimited worktree listing, verifies
the checkout's symbolic HEAD and shared Git directory, and refuses missing,
ambiguous or foreign checkouts. It does not switch branches or create a target
checkout on the user's behalf. Paths with spaces, Unicode and, on Unix,
newlines are preserved.
[src: file: backend/src/core/worktree.rs:1189]

Normal integration, boot classification and Applying-origin recovery inspect
the target checkout's uncommitted files. Immediately before fast-forwarding,
`fast_forward_target_to` resolves that checkout again and verifies its expected
HEAD and cleanliness. A changed HEAD or dirty target is refused; unrelated
files in the primary checkout are neither cleaned nor used as a blocker.
Backup refs, review evidence and durable saga checkpoints remain unchanged.
[src: file: backend/src/core/worktree.rs:1360]

## Regression evidence

During KT-614 qualification, a clean target in a separate worktree was blocked
by three untracked user files in the primary checkout. The pre-fix tests also
proved the more serious clean-primary case: Git advanced the wrong branch
while the pinned target remained unchanged.

On 2026-09-07, both integration tests failed before the fix. After the fix, all
six `pinned_target_checkout` regressions passed, including boot recovery, public
apply-block recovery, unchanged primary files/branch, repeated terminal
integration, dirty/drifted target refusals and checkout identity/path handling.
The adjacent `--lib orchestration` suite passed 339 tests and
`--lib core::worktree::tests` passed 92 tests, with no failures or ignored tests.
Formatting, diff hygiene and strict all-target clippy also passed.

These are disposable Git/SQLite tests. They do not mutate the operator's
checkout or prove that an already-running backend has loaded the new binary.
Full-release qualification remains a separate gate.

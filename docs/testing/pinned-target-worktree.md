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
[src: file: backend/src/core/worktree.rs:1930-1962#target_branch_checkouts]
[src: file: backend/src/core/worktree.rs:1964-2006#integration_target_worktree]

Normal integration, boot classification and Applying-origin recovery inspect
the target checkout's uncommitted files. Immediately before fast-forwarding,
`fast_forward_target_to` resolves that checkout again and verifies its expected
HEAD and cleanliness. A changed HEAD or dirty target is refused; unrelated
files in the primary checkout are neither cleaned nor used as a blocker.
Backup refs, review evidence and durable saga checkpoints remain unchanged.
[src: file: backend/src/core/worktree.rs:2211-2231#fast_forward_target_to]

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

## Target checked out nowhere (KT-835)

`integration_target` resolves one of two shapes. *Checked out once* keeps the
behaviour above, unchanged. *Not checked out* (no worktree has the branch) reads
the tip from `refs/heads/<branch>`, has no working tree to be dirty, and phase 2
is `is_ancestor(expected_tip, candidate)` then
`git update-ref -m <reason> refs/heads/<branch> <candidate> <expected_tip>`: git
refuses when the ref moved since the review (the compare-and-swap), and the ref
is read back afterwards. No checkout's working tree or index is read or written.
Several checkouts are still refused.

The shape is read again inside `fast_forward_target_to`, so a target that became
checked out between phases takes the checkout guards (a ref moved under a
checkout would leave its index and files stale). Every recovery path classifies
"not checked out" as clean: `integration_recovery_decision`, the
`run_integration` preflight, `finish_recovered_apply` and `resume_blocked_apply`
(rebuild on drift, idempotent close when the candidate already landed, apply when
the tip is still the pinned one). Tests: `unchecked_out_target_*` in
`core::worktree::tests` and `an_unchecked_out_target_*` in
`api::orchestration::tests`.

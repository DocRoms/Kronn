# TD-20260926-integration-target-update-ref

- **ID**: TD-20260926-integration-target-update-ref
- **Area**: Backend (task orchestration, `TwoPhaseFfOnly` integration)
- **Problem (fact)**: `task_exec` integration only advances a target branch
  that is checked out in exactly one worktree (KT-751):
  `worktree::integration_target_worktree` refuses zero or several checkouts,
  and phase 2 fast-forwards inside that checkout with `git merge --ff-only`.
  An autoCode flow therefore keeps one worktree per ticket just so its target
  is checked out. KT-798 item 4 asked to advance a target that is not checked
  out with `git update-ref refs/heads/<b> <candidate> <expected_tip>`, which
  is a compare-and-swap. It was not done in KT-798.
  `[src: file: backend/src/core/worktree.rs:1298-1339]`
  `[src: file: backend/src/core/worktree.rs:1429-1457]`
- **Why we can't fix now (constraint)**: changing only `fast_forward_target_to`
  is not enough. Five other places in the saga require the checkout and treat
  its absence as a refusal or an unknown state, and each has recovery tests:
  - `integration_recovery_decision` (recovery classification after a crash):
    a missing checkout makes `dirty` default to `true`, so it parks for a human.
    `[src: file: backend/src/api/orchestration.rs:713-795]`
  - `run_integration` preflight: refuses with `IntegrationTargetNotCheckedOut`
    before any candidate is built, then checks that checkout for dirty files.
    `[src: file: backend/src/api/orchestration.rs:2318-2345]`
  - `validate_and_apply`: phase 2 through `fast_forward_target_to`, and the
    refusal fix text from `apply_refusal_fix` / `target_checkout_fix`.
    `[src: file: backend/src/api/orchestration.rs:2136-2178]`
    `[src: file: backend/src/api/orchestration.rs:2472-2490]`
  - `finish_recovered_apply`: reads the real tip from the checkout's HEAD and
    re-checks it before the recovered fast-forward.
    `[src: file: backend/src/api/orchestration.rs:2814-2910]`
  - `resume_blocked_apply`: requires the checkout and refuses when it is dirty.
    `[src: file: backend/src/api/orchestration.rs:8059-8070]`
- **Impact**: dev friction (one kept worktree per target), disk use.
- **Where (pointers)**: the files above; the saga's recovery contract is
  `crate::models::saga_resume_action`.
- **Suggested direction (non-binding)**: introduce a target abstraction with two
  shapes. *Checked out once*: today's behaviour, unchanged. *Not checked out*:
  tip read with `rev-parse refs/heads/<b>`, nothing to be dirty, phase 2 =
  `is_ancestor(expected_tip, candidate)` then
  `git update-ref -m <reason> refs/heads/<b> <candidate> <expected_tip>` (git
  refuses if the ref moved: that is the CAS), read back afterwards. Keep
  refusing several checkouts. Recovery must classify "not checked out" as clean
  rather than unknown. A target that becomes checked out between phases must
  fall back to the checkout path (a ref moved under a checkout leaves that
  checkout's index and files stale). Every existing pinned-target test must
  still pass, plus one crash test per new branch.
- **Next step**: create ticket.

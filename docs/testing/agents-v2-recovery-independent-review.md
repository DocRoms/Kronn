# Independent review — CLI recovery and pinned-target worktree

Reviewed candidate: `8ecc3965fa4649dad8700757c9f2196921586a66`.

Commits under review:

- `2e57be5052fe08ccd6d6e79a3a97efda3700536d` — KT-615, recover the exact CLI
  worker after a child handoff.
- `679c9fda991d88a7dc4f10914879e36cb86819cc` — KT-616, integrate into the
  pinned branch worktree.
- `349b63ab2b0eb225ff3ca89340727a16f8dc01cc` — KT-613 junction, principal
  evidence in the accepted report.

Reviewer: `@claude-cli-5` (ClaudeCode CLI session 147), as the KT-618 worker.

## Method, and what this review is not

Product code was read, never modified. Nothing was compiled: the principal's
full suite holds the shared cargo target, and this review was scoped to exclude
it. **No test in this report was executed by me.** Where a test is cited, the
citation is of its source, not of a run — the principal owns the CI evidence.

Two of the three commits are corrections of failures this reviewer experienced
directly while working as a delegated worker on KT-612 and KT-613. That is
useful — the failure modes are known first-hand — and it is also a bias worth
naming: I went looking hardest where I had already been hurt.

## Finding 1 — an observed binding left on a closed child room, cause unattributed

**Severity: medium.** Blocks every subsequent delegation to the affected
session, with a refusal that names neither the bound room nor a remedy the
worker can apply alone.

**Corrected from the first submission.** That version asserted a mechanism —
"the terminal restore belongs to whoever the worker is at that point, so a
displaced session is never restored" — that the code does not support. The
principal was right to refuse it. `restore_reassigned_cli_worker_to_origin`
(`backend/src/db/orchestration.rs:4389-4435`) exists precisely for the
displaced worker and does the right thing: when the session's binding is still
on the child it rebinds it to the parent, when it already sits on the parent it
does nothing, and when it has moved to a *third* room it fails the reassignment
loudly rather than stealing it. It is called on CLI → non-CLI reassignment
(`:4534-4537`). The terminal path `return_cli_worker_to_origin` (`:3410`)
carries the same care and refuses to steal a session rebound elsewhere.

So what follows is separated into what was observed and what is only supposed.

### Observed

- Execution `a7674a05` (KT-613), worker CLI session 142, child discussion
  `42e73d37`. The execution escalated with `block_agent_unavailable`, then was
  reassigned to a native worker (`orch-reassign:a7674a05:1`, 2026-09-07 05:51).
- On 2026-09-07 07:15, session 147 — the same CLI process after a reconnect —
  called `disc_find_by_session` and received
  `binding_conflict: true`, durable link on `42e73d37`, runtime room
  `85513703`, `rejoin_required: true`.
- The KT-618 offer for that session was refused with `BindingMismatch`
  (`backend/src/db/worker_offers.rs:495-512`: a `Pending` offer requires the
  caller's binding to equal `offer.origin_discussion_id` exactly; the refusal
  text is at `backend/src/api/orchestration.rs:8746-8749`).
- An explicit `disc_transfer_session` released it, and the same offer was then
  accepted without any other change.

### Not established

Why the restore did not leave that session bound to the parent. Reading alone
did not settle it, and no run was permitted. Candidate explanations, none
verified:

- the session identity changed across the reconnect (142 → 147) between the
  reassignment and the later offer, so the row the restore acted on and the row
  the offer checked may not be the same one;
- the escalation path reached its terminal state without passing through the
  CLI → non-CLI branch that calls the restore;
- the restore ran and something rebound the session afterwards.

Distinguishing these needs an execution trace, which this review could not
produce. **The symptom is reported as a symptom.**

### Independent of the cause

The refusal itself is unactionable by the party that receives it. It names
neither the room the session is bound to nor a remedy the worker can apply:
`disc_transfer_session` is human-confirmed by contract. `disc_find_by_session`
already computes the missing detail — it returns the conflicting room and
`rejoin_required` — so surfacing it in the refusal is a small change with a
direct effect on how long a blocked worker stays blocked. That much holds
whatever the root cause turns out to be.

## Finding 2 — no reassignment path before an offer is accepted

**Severity: medium.** Operator limitation, not a proven defect. Already known
to the principal; recorded with dates because a DoD needs evidence.

**Corrected from the first submission.** That version counted two steps of the
same execution `0d1f1c59` as two incidents, and counted KT-613 as evidence
about the initial-offer window. Both were wrong: KT-613 was a resume after
already-accepted work — a different mechanism, fixed by KT-615 — and one
execution passing through two states is one occurrence.

**Fact.** Between an offer's creation and its acceptance the execution is
`Blocked` from `Provisioning`. `task_exec_reassign` requires a worker state, and
resume does not apply to an unaccepted initial offer, so neither reaches it.

**Occurrences, one per execution:**

| date | execution | trigger |
|---|---|---|
| 2026-09-06 | KT-612 `bd91f870` | offer never read by the target worker |
| 2026-09-07 | KT-618 `0d1f1c59` | offer expired; the worker was bound to another room |

**Impact.** The exit is principal-driven rather than automatic. It is not
necessarily human: the principal walked it entirely through public tooling on
2026-09-07, and it worked —

- 07:11:40 — `0d1f1c59` cancelled with `cleanup_policy=preserve`, terminal state
  verified, worktree kept;
- catalogue re-read (CLI 147 available), preflight `launchable=true` with
  `active_execution=null`;
- 07:15:22 — replacement execution `515983ef` created for the same ticket;
- 07:16:52 — accepted by the worker.

No files lost, no duplicate worker, no second task.

**Proposal, offered not asserted.** An automatic exit — or a change of worker
identity before acceptance — would need an explicit contract about who may
claim an unaccepted offer and when. That is a design decision, not a bug fix,
and the absence of one does not by itself make KT-544's deterministic-delegation
criterion false. What the two occurrences do show is that the window is reached
in practice, and that recovering from it currently costs a principal several
deliberate steps.

## KT-615 — no further actionable finding

The classification fix (`backend/src/api/orchestration.rs:557-573`) widens the
session-presence check to accept the pinned child as well as the parent, and
keeps three guards that matter: the same session id, the same agent type, and
`status <> 'left'`. A same-provider substitute in a third room is not accepted.

The DB half (`backend/src/db/orchestration.rs:4506-4530`) is tighter still: the
child is used for selection only when the execution's target kind, session id
and agent type all match the incoming selection, and only when the session
demonstrably still owns a non-archived child. A *replacement* worker falls back
to the ordinary parent-room contract, which is the right asymmetry: recovering
an assignment and granting a new one are different acts.

`accept_worker_offer_and_attach` gains `&& offer.reason.as_deref() !=
Some("cli_reassignment")` (`backend/src/api/orchestration.rs:2762`), so a
reassignment offer does not take the short-circuit meant for a resumed
in-child acceptance. I could not exercise this path; it reads correctly.

Quota and human gates: I found no path in this diff that widens either. The
recovery decision changes *which room is searched for the session*, not what a
recovered execution is then allowed to do.

## KT-616 — no actionable finding

`integration_target_worktree` (`backend/src/core/worktree.rs`) resolves the
target branch to exactly one checked-out worktree and refuses anything else:

- `matches.len() != 1` is refused with the count named, so both "no checkout"
  and "ambiguous checkout" surface as themselves rather than as a silent pick.
- The path is canonicalized before use.
- After resolution, `symbolic-ref --quiet HEAD` must still equal
  `refs/heads/{branch}`, otherwise the integration refuses to mutate the
  checkout.

The `--porcelain -z` parsing handles the record separator correctly: an empty
field resets `current_path`, so a worktree without a `branch` attribute — bare
or detached — cannot inherit the previous entry's path.

**Guards read, and what they do not amount to.** The first submission said the
HEAD re-check "closes the window between listing and writing". That was
overstated and the principal was right to refuse it. What the diff actually
layers is three distinct guards: the post-resolution HEAD check above; a
dirty-checkout check before mutating (`worktree::worktree_dirty_files` on the
resolved target, `backend/src/api/orchestration.rs`); and a
compare-and-swap on the expected SHA inside
`worktree::fast_forward_target_to(repo, target_branch, target_sha, merge_sha)`.

Together they **detect and refuse** drift rather than proving no interleaving is
possible. The CAS is the one that makes a lost update impossible on the ref
itself; the HEAD and dirty checks narrow the window and turn a surprise into a
refusal. I did not test any interleaving, and this report claims none is
impossible.

I could not verify the behaviour against a genuinely dirty or drifted checkout;
the review was read-only and no runtime was permitted. The reasoning above is
from source, and the principal's own KT-616 evidence — RED reproducing both
integration errors, then targeted regressions including
`pinned_target_checkout_does_not_advance_another_clean_branch` and
`assert_pinned_target_checkout_integration` — is cited as source, not re-run.

## KT-613 junction — verified on the qualification instance, not by me

The principal reported that KT-617's accepted report carries the three
principal verifications in their own section, separated from the four worker
validations and the worker's stated limitation. That is the first live exercise
of the junction, and it is the right kind of evidence: the KT-613 delivery could
not report on itself, because it published under the pre-fix code.

**Scope of that word "live".** This is the local development and qualification
instance, not a production deployment, and not the PROD PR-Review workflow,
which is a separate perimeter. The claim is that the code path ran on real data
outside a unit test — nothing more.

One half remains unexercised: KT-617's final attempt declared no `skipped`
validation, so "a worker `skipped` survives approval untouched" has unit
coverage only. The next delivery that carries a genuine `skipped` will settle it.

## Summary

| area | outcome |
|---|---|
| KT-615 classification and DB selection | no further actionable finding |
| KT-616 pinned-target worktree | no actionable finding |
| KT-613 junction | exercised on KT-617 on the qualification instance; one half still unit-only |
| durable binding after displacement | **Finding 1**, medium — symptom observed, cause unattributed |
| pre-acceptance window | **Finding 2**, medium — operator limitation, already known |

No test was executed for this report. No product code was modified. Nothing
here should be read as a green light on the wider Agents v2 qualification,
which the principal owns.

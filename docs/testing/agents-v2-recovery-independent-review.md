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

## Finding 1 — a displaced worker keeps a durable binding to a closed child room

**Severity: medium.** Blocks every subsequent delegation to that session, with
a refusal that names neither the cause nor the remedy.

**Where.** `backend/src/db/worker_offers.rs:495-512`. Accepting a `Pending`
offer requires the caller's durable source-session binding to equal the offer's
origin discussion exactly:

```rust
let binding_ready = match offer.status {
    WorkerOfferStatus::Pending => {
        bound_disc.as_deref() == Some(offer.origin_discussion_id.as_str())
    }
```

**Scenario, reproduced twice on 2026-09-06 and 2026-09-07.**

1. A CLI session accepts a worker offer. Its durable binding moves into the
   execution's child room.
2. The execution is taken away from it — escalated then reassigned to another
   worker, or cancelled before acceptance. The terminal return-to-origin event
   belongs to whoever the execution's worker is at that point, not to the
   displaced session.
3. The child room closes. The displaced session's durable binding still points
   at it.
4. The next offer, in the parent room, is refused with
   `BindingMismatch` → *"this CLI session is not durably bound to the offer
   room; reconnect or explicitly transfer the session, then retry"*
   (`backend/src/api/orchestration.rs:8746-8749`).

The message is accurate and unactionable: it does not say which room the
session is bound to, and the remedy it names — `disc_transfer_session` — is
human-confirmed by contract, so the worker cannot apply it alone.

**What does not resolve it.** `disc_leave()` on the child sets
`discussion_sessions.status = 'left'` but leaves the durable binding in place.
The session is then in the worst of both states: not recoverable by KT-615's
new check, which requires `status <> 'left'`
(`backend/src/api/orchestration.rs:557-573`), and not offerable elsewhere,
because the binding still points at the closed child. `disc_find_by_session`
reports this precisely as `binding_conflict: true` with
`rejoin_required: true` — the diagnosis exists; nothing acts on it.

**Uncertainty, stated.** A terminal execution does restore the binding to the
parent — `backend/src/api/orchestration.rs:15311` asserts exactly that. I could
not determine by reading alone whether that restore is skipped for a *displaced*
worker or whether it runs and is later overwritten. Both are consistent with
what I observed; distinguishing them needs a run I was not permitted to make.
The finding stands either way: the observed end state is a binding on a closed
room, twice.

**Suggested shape, not a prescription.** Either release the displaced session's
binding when an execution changes worker, or let the refusal carry the bound
room id so the worker can ask for the right transfer instead of guessing.

## Finding 2 — the pre-acceptance window has no automatic exit

**Severity: medium.** Already known to the principal; recorded here with dates
because a DoD needs evidence, not recollection.

Between an offer's creation and its acceptance the execution is `Blocked` from
`Provisioning`. In that state it has no active worker, and neither resume nor
reassign applies: `task_exec_reassign` requires a worker state, and resume does
not cover an initial offer. The only exit is a human-driven cancel-and-recreate.

Four occurrences, all in two days:

| date | execution | trigger |
|---|---|---|
| 2026-09-06 | KT-612 `bd91f870` | offer never read by the target worker |
| 2026-09-06 | KT-613 `a7674a05` | session in child read as worker gone — **fixed by KT-615** |
| 2026-09-07 | KT-618 `0d1f1c59` (1) | offer expired while the worker was bound elsewhere |
| 2026-09-07 | KT-618 `0d1f1c59` (2) | `Blocked` before acceptance, therefore not reassignable |

KT-615 removed one cause. The window itself remains, and each new cause lands
in it the same way. This is offered as an argument that KT-544's
"deterministic delegation" is not yet true, not as a criticism of the fixes:
both of them are correct as far as they go.

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
- **HEAD is re-verified after resolution** — `symbolic-ref --quiet HEAD` must
  still equal `refs/heads/{branch}`, otherwise the integration refuses to mutate
  the checkout. This closes the window between listing and writing, which is the
  defect a naive implementation would have.

The `--porcelain -z` parsing handles the record separator correctly: an empty
field resets `current_path`, so a worktree without a `branch` attribute — bare
or detached — cannot inherit the previous entry's path.

I could not verify the behaviour against a real dirty or drifted checkout; the
review was read-only and no runtime was permitted. The reasoning above is from
source, and the principal's own KT-616 evidence (RED reproducing both
integration errors, then 6 targeted regressions) is cited but not re-run here.

## KT-613 junction — verified in production, not by me

The principal reported that KT-617's accepted report carries the three
principal verifications in their own section, separated from the four worker
validations and the worker's stated limitation. That is the first live exercise
of the junction, and it is the right kind of evidence: the KT-613 delivery could
not report on itself, because it published under the pre-fix code.

One half remains unexercised: KT-617's final attempt declared no `skipped`
validation, so "a worker `skipped` survives approval untouched" has unit
coverage only. The next delivery that carries a genuine `skipped` will settle it.

## Summary

| area | outcome |
|---|---|
| KT-615 classification and DB selection | no further actionable finding |
| KT-616 pinned-target worktree | no actionable finding |
| KT-613 junction | verified live on KT-617, one half still unit-only |
| durable binding after displacement | **Finding 1**, medium |
| pre-acceptance window | **Finding 2**, medium, already known |

No test was executed for this report. No product code was modified. Nothing
here should be read as a green light on the wider Agents v2 qualification,
which the principal owns.

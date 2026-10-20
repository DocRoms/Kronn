# Audit run states and the action each one offers

This page lists the states an audit run (`audit_runs.status`) can be left in,
how each one is reached, and the action the audit timeline offers in it. Rule
(KT-1134, KT-1169): no state leaves the user without an action, and nothing
reads "running" once the run has stopped.

## States of a run

| Status | Reached by | Left by |
|---|---|---|
| `Running` | A launch takes the project's audit lease, then inserts the row. | The terminal write, a cancel, the drop guard, the boot reconcile, the cleanup action. |
| `Completed` | Every step succeeded, was `unchanged`, or was accepted; a Full run also wrote its drift baseline first. The status and the validation discussion are written in one transaction. | Terminal. A new run supersedes it. |
| `Interrupted` | At least one step is left to redo, or the run stopped early (drop guard, restart, cleanup), or the user cancelled a run that had completed a step (KT-1211, `cancelled_at_step` set). | Resume (every failed step), single-step retry, or acceptance then finalization. Each one is a new run that carries the done steps. |
| `Cancelled` | The worker acknowledged a cancel, and no step had completed yet (KT-1211). Carries `cancelled_at_step`. | Terminal. A new run supersedes it. The timeline says « Audit annulé le … pendant l'étape N » and offers « Relancer l'audit ». |
| `Failed` | Catastrophic failure of every step. | Terminal. A new run supersedes it. |

Sources:

- The lease is the single gate of every launch. [src: file: backend/src/api/audit/full.rs:433-440#try_acquire_lease]
- The row is inserted `Running`. [src: file: backend/src/db/audit_runs.rs:17-19#insert_running]
- The drift baseline is written before the terminal write, and only by a Full run; a baseline failure keeps the run `Interrupted`. [src: file: backend/src/api/audit/full.rs:3465-3471#baseline_ok] [src: file: backend/src/api/audit/full.rs:3769-3774#baseline_ok] [src: file: backend/src/api/audit/full.rs:4527-4533#should_write_full_baseline]
- `Completed` or `Interrupted`, with the validation discussion, is one transaction. [src: file: backend/src/api/audit/full.rs:4234-4240#write_terminal_state]
- `Interrupted` only moves a `Running` row. [src: file: backend/src/db/audit_runs.rs:893-900#mark_interrupted]
- The worker marks a cancel before it acknowledges it. [src: file: backend/src/api/audit/full.rs:1400-1437#mark_cancelled_at_step] [src: file: backend/src/db/audit_runs.rs:184-213#mark_cancelled_at_step]
- At boot, every `Running` row is a zombie and becomes `Interrupted`. [src: file: backend/src/db/mod.rs:276-308#reconcile_stale_runs] [src: file: backend/src/db/audit_runs.rs:910-941#reconcile_stale_runs]
- A run is resumable when it is the project's latest, `Interrupted`, with at least one done step. [src: file: backend/src/db/audit_runs.rs:1006-1008#latest_resumable]

## What counts as a done step

A step is done when it succeeded, ended `unchanged` (KT-1130), or was accepted
by a human (KT-1131). An accepted step keeps its failure on record:
`cli_success` stays false and `step_warning` keeps the reason. It gains
`accepted_at`, `accepted_by` and `accepted_reason`.

- Done steps, as a resume reads them. [src: file: backend/src/api/audit/full.rs:4305-4327#already_succeeded_step_indices]
- Per-run outcomes keep `succeeded`, `unchanged`, `accepted` and `failed` apart. [src: file: backend/src/api/audit/full.rs:4018-4036#RunProgress]

## Citation markers left after the last attempt (KT-1370)

A step that still holds citation markers after its last attempt, which neither
the agent nor the exact fixes of KT-1353 could repair, no longer ends the run
`Interrupted`. docroms step 8 showed the cost: a marker citing `config/packages/prod/monolog.yaml` without a line
(a text file cited without a line, which Kronn never guesses a line for)
survived three attempts, `fresh_full` was interrupted, and validate,
revalidate and partial were refused downstream. Romu decided (19/10) that
this degrades and continues.

- **When.** Only on the last attempt (`attempt >= max_attempts`), and only
  when every blocker of the step is a `broken_citation` (a malformed,
  unresolvable or out-of-range `[src: file: …]` marker). A missing or empty
  output, a raw placeholder, the coverage matrix, a detector signal, the remit,
  a broken link or an unbacked `deps:` / `url:` provenance keeps today's
  behaviour: the step fails and the run is interrupted. All or nothing: if one
  marker cannot be demoted, or the document still fails after demoting, the
  originals are put back and the step fails as before.
  [src: file: backend/src/api/audit/document_repair.rs:541-613#demote_last_attempt]
- **What it writes.** a marker on path `X` becomes `` `X` (unverified reference) ``
  (line, range, anchor and `:exists` dropped; a bundle becomes one flagged list).
  Nothing else of the marker survives: a comment the agent put inside it is
  dropped for every file, never carried after the flag, and a later piece of a
  bundle is kept only when it is cited like a path (line, range, `:exists`) or
  is a file of the project. The list (event, `step_warning`, notes, prompt)
  records only the path part (`X`), never the line or the comment. Inline code and
  fenced code are untouched, as in `mechanical_fix`. The flag is plain text:
  a backticked path is not a citation, so the gate never reads it and a
  revalidation reports the demoted references as they stand, never failing on
  the flag.
  [src: file: backend/src/core/document_optimization/demotion.rs:37-52#demote_unverifiable]
- **A secret file is never written.** Its bare path or line is already repaired
  to `:exists` by KT-1353. If a marker of a secret file is still there, it is
  removed entirely: no path, no line, no comment that could carry content. A
  marker counts as secret when it names or resolves to a secret file ANYWHERE
  in it: every stretch that starts at the beginning or after a space, a comma
  or a colon and ends at a space, a comma, a colon or the end (so any word of a
  comment, and any quoted path with a space), stripped of quotes, brackets and
  the like, in the written, the percent-decoded (several rounds) and the
  line-less form, judged
  by `is_secret_path` on the project-relative path, so a link
  (`key-view.txt -> ../.ssh/key`) or a chain of links through a secret
  directory (`keys -> .ssh -> vault`) is as secret as its target. The whole
  cited path is judged, spaces included, so a link whose name holds a space
  (`key view.txt -> ../.ssh/key`), quoted or not, is caught too. One secret
  piece drops the whole marker, bundle remainder included. It is listed, in the
  `documentary_demoted` event, `step_warning` and the notes, only as the fixed
  placeholder `[src: file: <secret file>]`.
  [src: file: backend/src/core/document_optimization/demotion.rs:19#SECRET_MARKER]
- **Nor copied to Kronn's state.** Before a citation repair or a demotion,
  `preserve_original` stores the document under `citation-originals/` (and
  recovery snapshots the same way). The copy has every such marker replaced by
  the placeholder (`scrub_secret_markers`), so no secret text survives under
  the docs root or the audit state.
  [src: file: backend/src/api/audit/document_repair.rs:646#preserve_original]
- **The step is done with a warning**, not failed and without a partial banner:
  `cli_success` stays true and `step_warning` lists every demoted reference
  (document, path part of the marker). The step emits `documentary_demoted` and its
  `step_done` carries `unverified_references`.
  [src: file: backend/src/api/audit/document_repair.rs:521-536#demotion_notes]
- **The run says how many.** The `done` event carries `unverified_references`;
  `audit_runs.documentary_notes` gains `unverified` (step, document, marker),
  in the Full pipeline at the final gate and in the partial one after each step
  that demoted (appended to what a resumed run already listed). The audit
  timeline lists them next to the other documentary notes, and the
  validation prompt names them for the human.
  [src: file: backend/src/api/audit/document_repair.rs:414-450#notes_json]
- **Both pipelines.** The Full pipeline demotes where its documentary gate
  would fail the step. The partial pipeline has no per-attempt documentary
  gate: it demotes where it fails a step for the citations left on its target,
  that is the enforce gate (enforce mode) and the stale-target check of a
  revalidation.
  [src: file: backend/src/api/audit/full.rs:2670-2672#demote_last_attempt] [src: file: backend/src/api/audit/drift.rs:1582#demote_last_attempt] [src: file: backend/src/api/audit/drift.rs:1638#demote_last_attempt]

## Recovering from failed steps

- **Resume** replays every failed step of the latest interrupted run in a new
  run. That run carries the done steps.
- **Retry one step** (`retry_steps` with `resume_run_id`) replays only the
  named steps. The other failed steps are copied as they ended and are not
  run (`step_skipped`, reason `not_retried`).
  [src: file: backend/src/api/audit/full.rs:4389-4414#retry_selection] [src: file: backend/src/api/audit/full.rs:1487-1498#not_retried] [src: file: backend/src/db/audit_runs.rs:1099-1126#carry_over_failed_steps]
- **Accept as it stands** is
  `POST /api/projects/{id}/audit-runs/{run_id}/steps/{step}/accept`. It is a
  human action only: the route is not on the bridge list, and the handler
  refuses any bridge caller. It is refused when the target is missing, empty
  or still holds template placeholders. It is also refused when the run is not
  the latest `Interrupted` run, or while the audit lease is held.
  [src: file: backend/src/lib.rs:1853-1858#accept_audit_step] [src: file: backend/src/api/audit/accept.rs:137-145#BridgeCaller] [src: file: backend/src/api/audit/accept.rs:59-81#check_target] [src: file: backend/src/api/audit/accept.rs:174-184#latest] [src: file: backend/src/api/audit/accept.rs:277-279#DocsWriteLease]
- Once no failed step is left, the resume runs no agent step and only
  finalizes the run. It goes to `Completed`, writes its validation discussion,
  which lists the accepted steps apart, and makes the project validatable.
  The validate gate still requires the latest run to be `Completed`.
  [src: file: backend/src/api/audit/validate.rs:185-186#RunNotCompleted]
- **An accepted step is never run again by a resume** (KT-1312). A blocker its
  document still holds does not queue it for documentary recovery; only
  `redo_steps` runs it again. The final documentary gate still judges the
  document, so a blocker left in it ends the run `Interrupted` until the
  document is fixed.
  [src: file: backend/src/api/audit/full.rs:622-625#documentary_recovery]
- **A blocker the documents held before the run never fails it** (KT-1353). A
  fresh Full run records the blocking diagnostics of the committed documents as
  *pre-existing* in the blame ledger (`.kronn/audit/document-blame.json`, step
  `0`) before step 1. The final gate reports them in a `documentary_preexisting`
  event (`phase: final_gate`) and a `preexisting_count`, and neither charges
  them to the founding step nor sets the run `Interrupted`; the recovery plan of
  a resume skips them. The exemption follows the blocker, not the document: a
  step that rewrites the document and leaves the citation as it was (a
  re-verification appending to `audit_history`, the africanews shape) did not
  bring it, so the blocker stays pre-existing. Only a step *handed* the blocker
  to fix is charged with it afterwards (`release`), and is then retried and
  redone like any other.
  [src: file: backend/src/api/audit/document_repair.rs:178#seed_preexisting] [src: file: backend/src/api/audit/document_repair.rs:247#release] [src: file: backend/src/api/audit/full.rs:3343#split_preexisting]
- **The exemption is the run's, not the ledger's** (KT-1353). The ledger stores
  the content hash of each pre-existing blocker's document as the run last left
  it (`seal`, after every step and after the final gate's own write). The
  validation reads the hashes before it rewrites anything and waves a blocker
  through only while its document is unchanged: an edit between the run and the
  validation (a validation agent's, a human's) hands that document's blockers
  back, so a citation fixed then brought back is refused. A ledger written
  without hashes honours nothing.
  [src: file: backend/src/api/audit/document_repair.rs:208#unchanged_since_run] [src: file: backend/src/api/audit/validate.rs:624#split_preexisting_in]
- **What the gate let through or repaired reaches the human** (KT-1353). The
  pre-existing blockers of the final gate and the exact replacements applied
  during the run are stored on the run (`audit_runs.documentary_notes`, JSON
  `{preexisting, autofixes}`), shown in the audit timeline ("N pre-existing
  citation problems in docs no step owns", "N exact gate replacements applied by
  Kronn", both with their list), and prepended to the validation discussion's
  first prompt, which tells the agent to list them to the user and that editing
  such a document makes its problems blocking again.
  [src: file: backend/src/api/audit/document_repair.rs:417#notes_json] [src: file: backend/src/api/audit/helpers.rs:463#documentary_notes_block] [src: file: backend/src/db/audit_runs.rs:225#set_documentary_notes] [src: file: frontend/src/components/AuditTimeline.tsx:53#parseDocumentaryNotes]
- **The blame ledger follows the blocker, not its wording** (KT-1353). A gate
  message restates the cited file's line count or a suggested path, so it
  changes when a later step rewrites that file. Two blockers are the same when
  their code, document and first quoted subject match (the cited path, and for
  an out-of-range line the file too); the step that first wrote the citation
  keeps the charge, so a final blocker written by step 8 is never reported
  against step 10.
  [src: file: backend/src/api/audit/document_repair.rs:31-50#stable_message]
- **A run whose failed steps are all accepted no longer blocks a refresh or a
  revalidation** (KT-1312): a resume would only finalize it. Any step still
  failed or never run keeps the refusal.
  [src: file: backend/src/api/audit/full.rs:4374-4387#resume_has_steps_left] [src: file: backend/src/api/audit/drift.rs:569-589#resume_has_steps_left]
- **Fingerprints follow the steps** (KT-1312): the docs a step rewrote are
  re-fingerprinted as soon as it ends, so a chain of resumed runs leaves no
  rewritten doc with the fingerprints of an earlier audit, whichever run
  finishes the chain.
  [src: file: backend/src/api/audit/full.rs:3119-3130#record_after_audit]
- **A doc under a partial banner stays in the plan** (KT-1312): the revalidation
  plan queues its owning step (or lists it as manual when no step owns it)
  and the badge stays raised, even when relevance sees nothing wrong with its
  citations. Accepting the step or a successful run clears the banner.
  [src: file: backend/src/api/audit/revalidate.rs:199-230#partial_docs] [src: file: backend/src/core/citation_drift.rs:498-518#partial_docs]

## Leases and agents that could block a project

- **Orphaned lease.** The drop guard settles a dropped run as `Interrupted`
  and then releases the lease. If the database write still fails after its
  retries, the lease stays held (fail-closed). It is also named in
  `AuditTracker::orphaned_leases`. The timeline reports it as
  `lease_orphaned`. The cleanup action (`POST /api/audit-runs/cleanup`) never
  touches a live audit. It settles the stale `Running` rows and releases the
  orphaned leases.
  [src: file: backend/src/api/audit/full.rs:125-165#orphaned_leases] [src: file: backend/src/lib.rs:282#lease_orphaned] [src: file: backend/src/api/audit/run.rs:293#lease_orphaned] [src: file: backend/src/api/audit/run.rs:165-197#audit_runs_cleanup] [src: file: backend/src/db/audit_runs.rs:962-1003#reconcile_running_except]
- **Validation agents.** Before an audit touches `docs/`, it stops any agent
  still answering in a validation discussion linked to one of the project's
  runs. While an audit of the project shows progress, such an agent is not
  started: its dispatch goes back to the queue with its attempts intact. A
  run clears its progress before it spawns its own validation agent.
  [src: file: backend/src/api/audit/full.rs:770-772#stop_validation_agents] [src: file: backend/src/api/audit/full.rs:223-258#stop_validation_agents] [src: file: backend/src/api/discussions/runtime.rs:493#hold_validation_during_audit] [src: file: backend/src/api/discussions/runtime.rs:1185-1188#hold_validation_during_audit] [src: file: backend/src/api/discussions/runtime.rs:1204-1225#progress] [src: file: backend/src/api/audit/full.rs:3889-3892#clear_progress]
- **Cancel.** A cancel waits for the worker to acknowledge it. Without that
  acknowledgement it answers with an error, and the UI keeps the audit shown,
  marked "cancel pending". It deletes only the validation discussion linked to
  the run it cancelled, never one an earlier run left behind.
  [src: file: backend/src/api/audit/full.rs:4703-4727#delete_run_validation_discussion] [src: file: backend/src/api/audit/full.rs:4789-4811#wait_for_cancel_ack]

## Launching from the existing state (KT-1169)

- **A launch never ignores an audit it could resume.** A Full or specialized
  launch without `resume_run_id` is refused while the project's latest run is
  resumable, unless the request carries `fresh: true`. That flag is the
  confirmed "run the whole audit again" choice. A stale-sections refresh is
  refused in the same case, as it would supersede the run, unless every failed
  step is accepted. The refusal names the
  run, so a bridge agent knows what to resume. A partial run, which the Full
  launch cannot resume, never refuses a launch.
  [src: file: backend/src/api/audit/full.rs:563-569#fresh_launch_refusal] [src: file: backend/src/api/audit/full.rs:4357-4387#fresh_launch_refusal] [src: file: backend/src/api/audit/drift.rs:586-593#fresh_launch_refusal]
- **The existing docs are never injected again.** The template is installed
  only into an empty AI doc folder, decided from its content rather than the
  displayed status; Kronn's own docs only get the template files they lack.
  A folder of human docs refuses a fresh Full launch, UI and MCP alike, before
  the run takes the lease, until the briefing chooses another folder or a move
  to `legacy/`. On a documented repository, every launch, fresh included,
  starts from the docs as they are: a filled document gets the
  re-verification prompt (KT-1132). A documented repository that never had the
  template v2 merge is offered that merge before its first launch from the
  card (KT-1181), and never before a resume.
  [src: file: backend/src/api/audit/full.rs:894-906#template_installed] [src: file: backend/src/api/audit/full.rs:402-433#sse_refusal] [src: file: backend/src/api/audit/full.rs:1539#reverifies_target]
- **Unknown is not "nothing to resume".** When the card cannot read whether a
  run is resumable, it says so and offers "Retry". Every launch button stays
  disabled until it knows. A step retried alone never falls back to a whole
  audit when its run is no longer resumable.
  [src: file: frontend/src/components/ProjectCard.tsx:529-538#resumableUnknown] [src: file: frontend/src/components/ProjectCard.tsx:888-894#retrySteps] [src: file: frontend/src/components/AuditTimeline.tsx:504-506#launchBlocked]

## Every error carries its action (KT-1169)

| Surface | Error | Action next to it |
|---|---|---|
| Timeline, audit phase | A launch or a resume stopped on an error: refused by the server, the stream closed, the network failed, or a step retried alone lost its run. | "Resume" replays that same launch, with the state of the moment it is clicked. A lost run is read again instead. |
| Timeline, audit phase | Whether a run can be resumed could not be read. | "Retry" reads it again. Launching waits. |
| Timeline, audit phase | The recorded runs could not be read. | "Retry" reads them again. The audit is never shown as not started. |
| Timeline, failed step | Acceptance refused, for example because the target still holds placeholders. | The form stays open with its reason. "Accept" tries again. |
| Timeline, lease | Orphaned lease. | "Unlock". |
| Timeline, live audit | Cancel not acknowledged. | "Retry the cancel". |
| Timeline, validation | The latest local run is not `Completed`, its state is unknown, or it has no linked validation discussion. | No Validate action is offered. The timeline explains which prerequisite is missing. |
| Recap panel | History or steps could not be read. | "Retry". |
| Template merge panel | Plan, start, accept or discard failed. | "Resume" replays that operation (KT-1181). |
| Verify-docs dialog | A stale-sections refresh failed. | The stale section stays in the single doc-state block. “Update affected steps” retries the partial run; “Verified, unchanged” settles it without an agent. Both remain unavailable while a resumable run takes priority. |
| Toasts during a run | A step failed or warned. | Informational: the run ends with that step's "Retry this step" and "Accept as it stands". |

- Launch error banner. [src: file: frontend/src/components/AuditTimeline.tsx:782-800#launchError] [src: file: frontend/src/components/ProjectCard.tsx:1074-1097#setAuditLaunchError] [src: file: frontend/src/components/ProjectCard.tsx:455#auditLaunchError]
- Unknown resumable state, unreadable timeline. [src: file: frontend/src/components/AuditTimeline.tsx:815-827#resumableUnknown] [src: file: frontend/src/components/AuditTimeline.tsx:828-838#loadFailed]
- Refused acceptance. [src: file: frontend/src/components/AuditTimeline.tsx:502-524#setAcceptError] [src: file: frontend/src/components/AuditTimeline.tsx:931-934#acceptError]
- Validation prerequisites and linked actions. [src: file: frontend/src/components/AuditTimeline.tsx:1031-1093#completedLocalRun]
- Verify-docs actions and resumable priority. [src: file: frontend/src/components/RevalidateAudit.tsx:395-426#onUpdateStale] [src: file: frontend/src/components/ProjectCard.tsx:1523-1535#primaryAuditAction]
- Recap read error. [src: file: frontend/src/components/AuditRecapPanel.tsx:363-368#errorBlock]

## “Verify docs” escalation (KT-1168, KT-1206, KT-1218)

“Verify docs” is the single entry point in the project header, the audit
timeline and a stale document's viewer. It has no lifecycle of its own.
[src: file: frontend/src/components/ProjectCard.tsx:1600-1614#project-audit-primary]
[src: file: frontend/src/components/AuditTimeline.tsx:566-571#onVerifyDocs]
[src: file: frontend/src/components/AiDocViewer.tsx:529-538#onOpenDocState]

- **Fail-closed doc state.** One block reports four counts: broken citations,
  citations that can be re-anchored deterministically, stale sections, and
  human-validated docs to reconfirm. Template placeholders keep their separate
  targeted fill action below the block. A missing drift, revalidation-plan or
  provenance response is shown as unknown, never clean or validated.
  [src: file: frontend/src/components/ProjectCard.tsx:1453-1468#docStateUnknown]
  [src: file: frontend/src/components/DocStateSummary.tsx:67-84#doc-state-counts]
- **Deterministic first.** Opening the entry point computes the citation plan
  without an agent. Re-anchoring moved citations is also deterministic.
  [src: file: frontend/src/components/RevalidateAudit.tsx:85-93#revalidationPlan]
- **Smallest sufficient fix.** Broken citations use the bounded agent pass;
  stale sections offer only their owning steps; unchanged sections can be
  acknowledged without an agent. A section can also need an agent because its
  prose changed even when it has no broken or moved citation; the panel says
  so explicitly. Unknown citation fingerprints keep the four-count state
  unknown, but do not block targeted agent steps returned by the plan. A full
  audit remains folded under an explicit advanced option with the last
  measured cost and duration, or an honest “not measured”.
  [src: file: frontend/src/components/RevalidateAudit.tsx:250-253#revalidate-prose-changed]
  [src: file: frontend/src/components/RevalidateAudit.tsx:345-353#hasTargetedAgentWork]
  [src: file: frontend/src/components/RevalidateAudit.tsx:354-364#docStateCounts]
  [src: file: frontend/src/components/RevalidateAudit.tsx:395-426#onUpdateStale]
  [src: file: frontend/src/components/RevalidateAudit.tsx:583-590#onFullAudit]
- **Human text survives.** A partial update reuses the saved briefing. A Kronn
  write to human-validated text applies the correction and marks that text
  “to reconfirm”; it does not silently replace the human decision.
  [src: file: backend/src/api/audit/drift.rs:447-474#briefing_notes]
  [src: file: backend/src/api/audit/drift.rs:1349-1357#mark_validated_doc_writes]
- **One explanation, three doors.** The header summary and primary action, the
  timeline's doc-state block and launch entry, and the banner of a stale
  document all open the same dialog: why the sections are stale, each doc
  with its audit step and changed sources, and what “Update” and “Verified,
  unchanged” do. The last commit that touched the changed sources is not in
  the drift response, so it is not shown.
  [src: file: frontend/src/components/__tests__/ProjectCard.stale-explanation.test.tsx:141-191#KT-1218]
- **One name on every surface.** The MCP descriptions and manuals say
  “Verify docs” like the English label, present the full audit as an advanced
  option, and carry none of the retired CTA names.
  [src: file: backend/scripts/test_disc_introspection_mcp.py:7985-8011#test_audit_surface_uses_the_verify_docs_vocabulary_of_the_ui]
- **Header layout.** The compact header keeps the status, one next action, the
  doc-state summary and the « Détails » disclosure without overlap at 360, 768
  and 1440 px, closed and open.
  [src: file: frontend/e2e/specs/audit-card-states.spec.ts:292-352#width]

- **Deterministic part.** It runs under the project's audit lease and leaves
  no run row. Moved citations outside human sections get their new line
  number, and their fingerprints move with them.
- **Agent part.** Docs with broken citations go to a `Partial` run over their
  owning steps, so the states above apply: Stop, `Interrupted`, the drop
  guard, and a validation discussion when the run completes.
- **Resuming.** Relaunching the action is the resume. The plan is recomputed
  from `docs/citations.json`, and the docs rewritten by a step that succeeded
  were re-fingerprinted even if the run was interrupted.
- **Validation.** A run that rewrote no agent target, because every step
  ended `unchanged` or was stopped first, restores the `validated_at` it
  revoked, like any partial run (see below).

Sources:

- The plan and the re-anchoring. [src: file: backend/src/api/audit/revalidate.rs:90-156#plan] [src: file: backend/src/api/audit/revalidate.rs:363-403#apply_reanchors]
- The partial pipeline the agent part reuses. [src: file: backend/src/api/audit/revalidate.rs:753-766#run_partial_pipeline]

## Refreshing stale sections (KT-1207)

A stale-sections refresh is a `Partial` run over the steps of the stale
sections.

- **Done steps.** A step is done when it succeeded or ended `unchanged`: an
  unchanged step re-verified its section. The drift baseline is merged for
  the done steps, whatever the run's status, after the terminal commit. A
  failed step's section stays stale.
  [src: file: backend/src/api/audit/drift.rs:1978-1986#done_steps] [src: file: backend/src/api/audit/drift.rs:2338-2346#compute_merged_baseline]
- **Status.** Without a failed step the run is `Completed`. The SSE `done`
  reads `complete` when a step rewrote its target, `no_change` when none did.
  With a failed step it is `Interrupted`.
  [src: file: backend/src/api/audit/drift.rs:2383-2413#done_status_of]
- **Validation.** The done steps get a validation discussion scoped to them,
  linked in the terminal transaction, on an `Interrupted` run too: the
  validate gate still refuses that run. A run that rewrote nothing, every
  step unchanged or stopped before its first write, restores the
  `validated_at` it revoked at launch, and then opens no discussion.
  [src: file: backend/src/api/audit/drift.rs:2014-2020#restore_validation_if_untouched] [src: file: backend/src/api/audit/drift.rs:2076#pending_validation] [src: file: backend/src/api/audit/drift.rs:2445-2460#Interrupted]
- **Citations.** A section with recorded citations is judged by them. A
  normal refresh re-fingerprints both rewritten and re-verified unchanged
  sections after the terminal commit; failed sections keep their previous
  fingerprints. Revalidation retains its explicit human acknowledgement for
  unchanged citations.
  [src: file: backend/src/api/audit/drift.rs:2209-2232#citation_docs]
- **« Vérifié, inchangé ».**
  `POST /api/projects/{id}/drift/acknowledge` refreshes the named sections'
  baseline mappings, and their citation fingerprints when recorded. It runs
  no agent and writes no doc. It is a human action only: the route is not on
  the bridge list, and the handler refuses any bridge caller. It takes the
  audit lease.
  [src: file: backend/src/lib.rs:1761-1764#acknowledge_drift] [src: file: backend/src/api/audit/drift.rs:102-107#bridge] [src: file: backend/src/api/audit/drift.rs:112-116#DocsWriteLease] [src: file: backend/src/api/audit/docs_write.rs:25-29#try_acquire_lease] [src: file: backend/src/core/checksums.rs:770-801#acknowledge_sections]

## The action offered in each state

The compact header exposes only the honest status, its primary next action and
the doc-state summary. One “Details” disclosure contains secondary metadata and
actions. Three pre-audit cases override the legacy `TemplateInstalled` label:
[src: file: frontend/src/components/ProjectCard.tsx:1483-1516#humanDocsDetected]
[src: file: frontend/src/components/ProjectCard.tsx:1690-1708#project-audit-details]

- a human-owned docs folder asks the user to choose where Kronn writes;
- a cancelled first run names its cancellation date and offers a relaunch;
- a legacy `ai/` layout asks for migration before any audit action.

| State (as the timeline sees it) | Primary action | Other actions |
|---|---|---|
| Never audited | Launch the audit | — |
| Running | Cancel | — |
| Cancel not acknowledged | Retry the cancel | The status poll clears it when the run stops. |
| `Interrupted`, steps to redo (first audit or re-audit) | Resume the failed steps | Per failed step: retry it alone, or accept it as it stands. Run the full audit again, behind a confirmation that offers the first two. The validation phase points to the same resume. |
| `Interrupted`, every failed step accepted | Finalize the audit | — |
| `Interrupted` with nothing done (restart during step 1) | Launch the audit | — |
| `Cancelled` before any completed run | Run the audit again, with the cancellation date in the header | — |
| `Cancelled` after a completed run | Verify docs | Full audit remains an advanced option. |
| `Completed`, linked validation unfinished | Resume the linked validation discussion | Verify docs. |
| `Completed`, linked validation finished | Validate | Open the linked discussion; Verify docs. |
| `Completed` without a linked validation | Explain that validation is unavailable | Never create an unlinked validation discussion. |
| Validated | Verify docs | Review the tech debt; full audit is an advanced option. |
| Orphaned lease | Unlock (cleanup) | Launch stays disabled until then. |
| Resumable state unknown | Retry the read | Launch stays disabled until then. |
| Any state, after a launch error | Resume that launch | Dismiss. |

The validation phase never reads "running" while an audit runs. It derives its
action from the latest local run of any kind and that run's `latest_validation`;
an older completed Full or Partial run cannot authorize Validate after a newer
specialized run. Discussion-title guesses never authorize Validate.
[src: file: frontend/src/components/ProjectCard.tsx:583-589#latest]
[src: file: frontend/src/components/AuditTimeline.tsx:332-342#latestRun]
[src: file: frontend/src/components/AuditTimeline.tsx:1084-1146#linkedValidation]

- Step status, accepted included. [src: file: frontend/src/components/AuditTimeline.tsx:205-220#stepStatus]
- Launch label: resume, finalize, run again. [src: file: frontend/src/components/AuditTimeline.tsx:488-495#launchLabel]
- Per-step actions only on the resumable run. [src: file: frontend/src/components/AuditTimeline.tsx:503#stepActions]
- Validation phase is not running during an audit. [src: file: frontend/src/components/AuditTimeline.tsx:508-509#validationRunning] [src: file: frontend/src/components/ProjectCard.tsx:678-683#validationInProgress]
- Validation phase hands over to the run. [src: file: frontend/src/components/AuditTimeline.tsx:1060-1073#runPending]
- Confirmation before a fresh full audit. [src: file: frontend/src/components/AuditTimeline.tsx:1269-1310#confirmRelaunch]
- Orphaned lease banner. [src: file: frontend/src/components/AuditTimeline.tsx:728-781#leaseOrphaned]
- Cancel pending. [src: file: frontend/src/components/AuditTimeline.tsx:691-692#cancelPending] [src: file: frontend/src/components/ProjectCard.tsx:453#cancelPending]

## Validation and document trust (KT-1206)

Validation requires the latest local run, of any kind, to be `Completed`.
Its linked validation agent must have stopped: a pending dispatch, an active
process, or `awaiting_agent` blocks signoff. Settled Critical/High debt sheets
replace the discussion's terminal marker only when at least one such sheet
exists. Medium/Low-only and empty debt lists do not replace that discussion.
An archived unfinished discussion may be replaced only by a finished one
created after the run started. Unknowns block every run kind; only Full runs
write the optimization report.
[src: file: backend/src/api/audit/validate.rs:433-442#is_active]
[src: file: backend/src/api/audit/validate.rs:517-527#validation_agent_active]
[src: file: backend/src/api/audit/validate.rs:348-357#td_gate_met]
[src: file: backend/src/api/audit/validate.rs:454-495#validation_discussion_finished]
[src: file: backend/src/api/audit/validate.rs:610-633#optimize_documents]

The completed run records SHA-256 fingerprints of the actual Markdown
documents, excluding `legacy/` and `var/`. An unreadable file or a symlink
fails capture. The linked validation agent records its final document contents
atomically with its successful terminal reply, after it stops and redaction
succeeds. A later added, deleted or changed document prevents signoff; neither
committed provenance nor a matching timestamp replaces this local proof.
Historical runs without fingerprints require a new audit before signoff.
[src: file: backend/src/core/audit_documents.rs:11-45#capture]
[src: file: backend/src/core/audit_documents.rs:51-65#verify]
[src: file: backend/src/db/discussions.rs:1877-1914#insert_native_agent_message_with_audit_documents]
[src: file: backend/src/api/discussions/streaming.rs:5079-5101#validation_documents]
[src: file: backend/src/db/sql/260_audit_document_fingerprints.sql:1-3#final_doc_fingerprints_json]

The explicit re-anchor action sends `deterministic_only: true`. It returns the
remaining plan without launching an agent, even when other sections still need
agent work; retry retains this mode. A later general fix is a separate choice.
[src: file: backend/src/api/audit/revalidate.rs:745-747#deterministic_only]
[src: file: frontend/src/components/RevalidateAudit.tsx:157-168#deterministicOnlyRef] [src: file: frontend/src/components/RevalidateAudit.tsx:216-223#deterministicOnlyRef]

Local TD decisions and deterministic citation repairs advance this proof only
for documents whose contents still matched it before the operation. Explicit
human confirmation advances only the selected documents and refuses unresolved
template placeholders or TODOs before accepting any selection. Ordinary Git
changes cannot certify themselves. Reverification also plans bounded agent
work for changed prose; deleted or unowned documents require manual attention.
[src: file: backend/src/db/audit_documents.rs:93-126#advance_local_changes]
[src: file: backend/src/db/audit_documents.rs:62-91#confirm_paths]
[src: file: backend/src/core/doc_provenance.rs:703-713#contains_placeholder]
[src: file: backend/src/api/audit/revalidate.rs:294-351#plan_with_documents]

A no-change partial run restores validation only when every tracked validated
document and every selected target remained byte-identical. The learning gate
does not treat a changed human-validated document, or one with broken citations,
as human-validated evidence until its trust conditions hold again.
[src: file: backend/src/api/audit/helpers.rs:1094-1119#validation_restore_snapshots]
[src: file: backend/src/core/doc_provenance.rs:777-815#source_provenance_with_citations]
[src: file: backend/src/core/learning_gate.rs:57-71#source_provenance_with_citations]

## Validation completion, reasons and provenance (KT-1122)

- **One parser, one verdict.** The terminal signal is the last line of an agent
  message, `**`, backticks and `_` tolerated. The backend parses it once; the
  UI reads `finished` from `GET /api/projects/:id/validation-state` (and the
  timeline's `latest_validation`). The only client-side copy is
  `endsWithValidationSignal`, for the per-message "view tech debts" button, on
  the same rule.
  [src: file: backend/src/db/audit_validation.rs:15-20#is_validation_signal]
  [src: file: backend/src/api/discussions/mod.rs:141-159#ends_with_terminal_signal]
  [src: file: frontend/src/lib/validationSignal.ts:18-22#endsWithValidationSignal]
- **Completion is durable.** The first clean signal stamps
  `audit_runs.validation_completed_at` and the id of that message, in the write
  path of the agent message (migration 262). Later messages (a commit note, a
  DoD update) never undo it; only relinking the run to a different discussion
  clears it.
  [src: file: backend/src/db/audit_validation.rs:95-108#on_agent_message]
  [src: file: backend/src/db/audit_validation.rs:77-93#record_completion]
  [src: file: backend/src/db/audit_validation.rs:110-123#effective_completion]
- **A reason, not a failing button.** The server states why a validation is not
  possible (`ValidationBlock`: no local run, audit running, run not completed,
  no linked discussion, discussion missing, foreign discussion, not finished).
  The timeline and the discussion banners show it instead of the action. A
  validation discussion that is not the latest run's is flagged as superseded.
  [src: file: backend/src/api/audit/validate.rs:35-59#ValidationBlock]
  [src: file: backend/src/api/audit/validate.rs:138-201#validation_state]
- **Peers are read-only, unless the repository carries the run.** An instance
  without the run (a P2P peer, another clone) is read-only: `validate-audit`
  refuses with `no_local_run`. The exception (KT-1199): the repository carries
  a committed, Completed run state that passes the gates — see
  [The run, portable by the repository](#the-run-portable-by-the-repository).
  Then the clone validates from that state, with the same gates as a local run.
  [src: file: backend/src/api/audit/validate.rs:35-59#ValidationBlock]
  [src: file: backend/src/api/audit/run_state.rs:321-345#repo_validation]
- **Who.** The operator pseudo of the instance that ran the audit is recorded
  as `validation_started_by` when the validation is opened, and as
  `validated_by` when it is validated, on the run row and in
  `docs/.kronn.json` (`validated_by`, `validation_started_by`).
  [src: file: backend/src/core/kronn_state.rs:330-346#mark_validated_by]
  [src: file: backend/src/db/audit_validation.rs:125-140#set_started_by]
- **Boot repair.** At startup, completions already visible in the messages are
  stamped, and open validations an older audit left behind are archived only
  when title and first message carry the generated provenance, they predate the
  latest linked run, and no agent is queued, running or awaited. Idempotent.
  [src: file: backend/src/db/audit_validation.rs:175-248#repair]

## The run, portable by the repository

KT-1199. The run, its steps and its resume point live in the local database;
the file `<docs>/.kronn-audit-run.json` projects them into the repository so a
colleague can see, resume and validate an audit that ran elsewhere. It sits
next to the docs (not under `.kronn/`, which is excluded from Git) so it
travels with them. [src: file: backend/src/core/audit_run_state.rs:27#RUN_STATE_FILENAME]

**What it holds.** The run id, kind, status, start and end dates, who started
it (the operator pseudo), the run it resumed, the template version and the
digest of `briefing.md` at the start; one line per step (`001`, `002`, …) with
its status (`done`, `unchanged`, `accepted`, `failed`, `pending`), its target
and the SHA-256 of the file it produced; and the KT-1174 baselines
(`decisions.md` and the `Decision record` lines at run start, redacted like
every audit artefact). No cost, no token count, no secret, no Kronn version
(it would change on every upgrade). Keys are sorted and the output is
byte-identical for the same input, so a merge or a rebase conflicts on a step
line at most.

**When it is written.** By Kronn at run start, after each step (done, failed,
cancelled) and at the end of the run (the end re-reads every produced file).
A failure to write is logged and never fails the run. A first audit writes it
once the docs folder exists. After a validation, the digests are rebuilt so
they follow the documents the human accepted: from this instance's rows when
it ran the audit, from the committed copy otherwise.
[src: file: backend/src/api/audit/run_state.rs:82-100#sync]
[src: file: backend/src/api/audit/run_state.rs:384-446#refresh_validated_digests]

**What is trusted.**

| Reader | Trust |
| --- | --- |
| The run in progress | Kronn's memory and database. The writer never reads the file back: an agent editing it mid-run changes nothing, the next step rewrites it. |
| A rescan, a resume, a validation | Only a file identical to `HEAD` (a human committed it). An uncommitted file is shown, flagged as not committed, and never used to resume or validate. |
| A step | Trusted when its file still has the recorded digest. A changed or missing file is « à revérifier »: never trusted, replayed on resume, and it blocks a validation from the repository. |

[src: file: backend/src/core/audit_run_state.rs:281-300#probe]

**What the UI shows.** `GET /api/projects/:id/audit-latest`,
`audit-timeline` and `audit-repo-state` carry the repository's run, read off
the working tree at every call, so a fetch, a rebase or a rescan shows up on
the next refresh: « audit lancé par X le JJ/MM : étapes 1-9 faites, 10-16 à
reprendre », and the tech-debt count. The repository's run is the project's
state when this instance has no run or the file is newer than its latest run
(a run still `Running` here is never superseded).

**Resuming.** A committed, unfinished run that is the project's state is
returned by `GET /api/projects/:id/audit-resumable` (`from_repository: true`),
so the usual « Reprendre » action sends its id as `resume_run_id`. The launch
rebuilds a local `Interrupted` run from the file: the steps whose produced file
still matches are inherited (no cost: this machine did not run them) and the
baselines are the committed ones; the pending, failed and changed steps are
replayed. The committed briefing is used.
[src: file: backend/src/api/audit/run_state.rs:448-453#adopt_repo_run] [src: file: backend/src/api/audit/run_state.rs:499-520#inherited]

**Validating.** With no local run (or a newer repository run), the validation
gate accepts the repository when: the file is committed; its run is
`Completed`; every step is trusted; every Critical and High tech-debt sheet is
settled; and the same documentary gates as a local run hold (no placeholders,
no blocking optimisation diagnostics). Whoever validates writes
`validated_at` in `.kronn.json`; the others receive it with the commit.
Otherwise the instance stays read-only and the reason is stated (`no_local_run`,
`repo_run_not_committed`, `repo_steps_to_recheck`, `repo_td_open`).
[src: file: backend/src/api/audit/validate.rs:224-265#repo_validation_state]

**Conflicts.** Two people auditing at once is resolved by Git. Whatever the
merged file says, a step whose digest no longer matches the merged document is
« à revérifier » and replayed; nothing is trusted on the file's word alone.

## Template updates after an audit

Kronn stores the upstream template version and per-file hashes in the docs
root's `.kronn.json` at install, reinstall and audit recording. The template
version is independent of the application's release version; the first tracked
catalogue uses `kronn:doc-version="1.1"`. A version or catalogue mismatch, or
missing files, stays visible even on an audited project. Filled project docs
are not compared against template bytes to decide whether an update exists.
[src: file: backend/src/core/kronn_state.rs:94-95#template]
[src: file: backend/src/core/template_manifest.rs:66-80#snapshot]
[src: file: templates/docs/AGENTS.md:1#kronn]

The project's update notice and MCP `audit_prepare.template_update` list missing
files with the current Full-chain steps that own them. Install the additive
update, then use the existing placeholder action to run only those steps.
Files without an owning step, including `environments.md`, workflow files and
`operations/mcp-servers.md`, are listed for manual completion; the registry does
not assign them an audit step.
[src: file: backend/src/core/template_manifest.rs:82-109#check]
[src: file: backend/src/api/audit/helpers.rs:173-178#template_update]
[src: file: frontend/src/components/TemplateUpdateNotice.tsx:34-40#installTemplate]
[src: file: backend/scripts/disc-introspection-mcp.py:2740-2748#audit_prepare]

A rename requires a unique match between a removed recorded template hash and
an added upstream file. The move keeps the project's bytes; an occupied target
or ambiguous successor is preserved for human reconciliation. No historical
rename is inferred for projects without a recorded manifest. The older
`ai/index.md` to `docs/AGENTS.md` layout conversion remains the separate legacy
migration, as recorded in the actual template history.
[src: file: backend/src/core/template_manifest.rs:110-146#renames]
[src: file: backend/src/core/template_manifest.rs:184-210#apply_renames]
[src: commit: 2ff8a178f]

The bundled anti-hallucination spec is refreshed only when its bytes match the
last recorded installed hash or an exact historical bundled revision. Local
edits survive and are reported. Reinstall uses the scanner's uppercase
placeholder grammar, so ordinary Vue, Jinja and Twig expressions are filled
content, not preselected damage.
[src: file: backend/src/core/template_manifest.rs:212-254#record_install]
[src: file: backend/src/api/projects/template_reinstall.rs:262-279#has_unfilled_placeholder]

Standalone install and reinstall snapshot provenance before writing and record
the resulting files afterward. Unreadable provenance refuses the operation
before any template write; a recording failure after writing is returned to the
caller. Both template writers use the existing `TemplateReinstall` provenance
category.
[src: file: backend/src/api/projects/template.rs:103-104#snapshot_tracked]
[src: file: backend/src/api/projects/template.rs:161-168#note_snapshot_writes]
[src: file: backend/src/api/projects/template_reinstall.rs:393-394#snapshot_tracked]
[src: file: backend/src/api/projects/template_reinstall.rs:504-512#note_snapshot_writes]

## Cancel and failed steps keep the docs whole (KT-1211)

- **A cancel after a completed step is resumable.** `mark_cancelled_at_step`
  settles the run `Interrupted` when `last_completed_step >= 1`, `Cancelled`
  otherwise, and records the step in flight in `audit_runs.cancelled_at_step`
  (0 = before step 1). Cancelling at step 12 resumes at step 12, not step 1.
  [src: file: backend/src/db/audit_runs.rs:184-213#mark_cancelled_at_step]
- **A stopped step puts its target back.** Before each step the target's
  content is kept in `audit_step_snapshots` (the `pre_snapshot` is only a
  hash). A cancel restores it, or removes a file the step created, and
  records `target_state = restored` with a `stopped during step N` reason.
  The partial pipeline does the same, with the same durable snapshot and the
  same process-group record. The outcome is never collapsed: `Restored`,
  `Untouched` (wrote nothing), `Partial` (kept under the marker) and
  `Unsettled` (could be neither restored nor marked: no `restored` state, so
  accepting it needs the override, and its snapshot is kept for the diff).
  A snapshot is capped at 4 MiB (a larger target is not snapshotted: boot
  cannot restore it, but its step still never ended and so still needs the
  override). A new run drops the snapshots of the project's earlier runs and
  deleting a run cascades.
  [src: file: backend/src/api/audit/step_guard.rs:129-154#restore_target] [src: file: backend/src/api/audit/step_guard.rs:246-263#KilledOutcome] [src: file: backend/src/api/audit/step_guard.rs:297-319#target_state] [src: file: backend/src/db/audit_runs.rs:281-314#MAX_SNAPSHOT_BYTES] [src: file: backend/src/db/audit_runs.rs:32-39#audit_step_snapshots] [src: file: backend/src/db/sql/265_audit_cancel_and_integrity.sql:20-31#audit_step_snapshots]
- **A crash is settled at boot, not at the next launch.** Right after the
  recorded agents are killed, every step that never ended and has a snapshot
  is put back (or marked) and recorded failed with a `stopped during step N
  (the backend stopped)` reason. Until then the doc would sit half-written
  with no marker. A step that never ended and has no verdict is a blocker of
  its own (`NeverEnded`): accepting it needs the override too.
  [src: file: backend/src/api/audit/step_guard.rs:587-614#settle_unfinished_at_boot] [src: file: backend/src/db/mod.rs:293-297#stop_agents_and_settle_at_boot] [src: file: backend/src/api/audit/step_guard.rs:926-941#AcceptBlocker]
- **Settling never destroys what the agent did not write.** The audit
  watches the target on the activity tick and keeps the fingerprint of what it
  saw the agent write (`audit_step_snapshots.observed_hash`, migration 264).
  A crash cannot tell a hand edit made before the next boot from the orphaned
  agent's own late write, so boot restores only content that matches that
  fingerprint (or is absent or blank). Anything else is left in place under a
  partial marker (`ChangedSinceSeen`, `target_state = partial`) and its
  snapshot is kept. A cancel is different: Kronn just killed the agent, so it
  restores, but content it cannot vouch for is first copied to
  `<target>.kronn-displaced-<UTC timestamp>` and the step's reason names the
  path; if the copy cannot be made the target is not replaced.
  [src: file: backend/src/api/audit/step_guard.rs:353-434#settle_killed] [src: file: backend/src/api/audit/step_guard.rs:156-189#save_aside]
- **Settling never writes outside the project.** Every read, restore, marker
  and displaced copy goes through the project-containment check (no symlink
  on the path, no `..`), temp files are created exclusively
  (`create_new`, `O_NOFOLLOW`) under a unique name, and no directory is created
  with `create_dir_all` beyond the project. A project root that does not exist
  (an unmounted volume) is never recreated: its steps stay open, snapshot
  kept, and are settled by the next boot or launch that finds the root.
  [src: file: backend/src/api/audit/step_guard.rs:60-75#contained_path] [src: file: backend/src/api/audit/step_guard.rs:86-101#sibling] [src: file: backend/src/api/audit/step_guard.rs:103-117#create_new_file]
- **File work stays off the writer connection.** The launch-time net reads
  the pending snapshots, settles the files, then records the verdicts, in three
  separate steps (`pending_steps`, `settle_pending`, `record_settled`).
- **A failed step marks what it left.** When a step fails after the agent
  wrote its target, Kronn keeps the text (a human may accept it) under a
  `<!-- kronn:partial-step -->` banner and records `target_state = partial`.
  A later success, or an acceptance, removes the banner.
  [src: file: backend/src/api/audit/step_guard.rs:16-22#PARTIAL_OPEN] [src: file: backend/src/api/audit/step_guard.rs:225-244#settle_failed_target] [src: file: backend/src/api/audit/step_guard.rs:212-223#clear_partial_marker]
- **Accept refuses an unsound step.** A step that failed for truncation,
  citations that do not resolve, an undisposed detector signal, incomplete
  coverage, or a stop is accepted only with `override_unsound: true`. The
  refusal starts with `override_required:` and carries a bounded diff against
  the pre-step content. The override is recorded in the acceptance reason.
  [src: file: backend/src/api/audit/step_guard.rs:923-941#AcceptBlocker] [src: file: backend/src/api/audit/accept.rs:34-36#OVERRIDE_REQUIRED_PREFIX] [src: file: backend/src/api/audit/accept.rs:261-275#OVERRIDE_REQUIRED_PREFIX]
- **Boot kills an agent the previous backend left.** The run records the
  agent's process group and the start stamp of its leader (`agent_pgid`,
  `agent_started_marker`). At boot, before the lease is freed, the group is
  killed only when the stamp still matches: a reused pid is never signalled.
  M5 was real: the agent is its own group leader (`setpgid(0, 0)`) and
  `kill_on_drop` is the only teardown, which cannot run when the backend is
  killed with SIGKILL. The stamp is the kernel start time (macOS), or the
  boot id plus the start tick (Linux: ticks restart at each boot).
  [src: file: backend/src/agents/runner.rs:13715] [src: file: backend/src/agents/runner.rs:13725] [src: file: backend/src/core/agent_orphans.rs:84-117#kill_recorded_group] [src: file: backend/src/core/agent_orphans.rs:23-73#start_stamp] [src: file: backend/src/db/mod.rs:288]
- **Source drift on a resume is saved.** `audit_runs.resume_drift` keeps what
  moved between the run a resume continues and the resume. The timeline shows
  it, the validation prompt carries it, and `redo_steps` (with
  `resume_run_id`) redoes the inherited steps it affects.
  [src: file: backend/src/api/audit/full.rs:1102-1116#set_resume_drift] [src: file: backend/src/api/audit/full.rs:608-634#redo_steps_req]

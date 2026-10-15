# Documentation writers and audit leases

Documentation mutations share the project's audit lease. `DocsWriteLease::acquire`
refuses an existing owner, then calls the validation-agent stop path before the
writer proceeds. The last guard releases the lease. A blocking worker owns a
clone, so dropping its HTTP waiter cannot admit another writer while the first
worker is still changing files.
[src: file: backend/src/api/audit/docs_write.rs:8-54#DocsWriteLease]

The guarded entry points include template install and reinstall, anti-hallucination
injection and redirector sync, documentation migration, and audit-state
attestation and restoration.
[src: file: backend/src/api/projects/template.rs:19-45#install_template]
[src: file: backend/src/api/projects/template_reinstall.rs:583-611#reinstall_for]
[src: file: backend/src/api/projects/anti_hallu_inject.rs:120-207#inject]
[src: file: backend/src/api/projects/migrate.rs:66-110#migrate_docs]
[src: file: backend/src/api/context_audit.rs:218-289#attest_project_documentation]

Briefing saves, reviewed briefing completion, and briefing/dispute card answers
use the same guard. A card decision keeps its guard inside the database worker
that applies the resulting filesystem changes.
[src: file: backend/src/api/audit/briefing.rs:46-81#set_briefing]
[src: file: backend/src/api/audit/briefing.rs:146-189#complete_reviewed_briefing]
[src: file: backend/src/api/audit/briefing.rs:724-767#save_briefing_answers]
[src: file: backend/src/api/discussion_questions.rs:18-68#dispute_lease]
[src: file: backend/src/api/discussion_questions.rs:201-239#answer]

Linked-repository updates hold the source project's lease and acquire each reverse
target's lease before syncing its `linked-repos.md`. A busy reverse target keeps
its existing file; the existing best-effort synchronization logs that refusal.
[src: file: backend/src/api/projects/crud.rs:708-869#set_linked_repos]

Validation admission checks the lease even when no audit progress exists. A
claimed validation job is returned to the queue without consuming an attempt.
After preflight, cancellation-token registration and a second lease check share
the tracker mutex, so a writer can either reject that admission or cancel its
registered token. A dispatch already stopped by a writer cannot start afterward.
[src: file: backend/src/api/discussions/runtime.rs:1188-1248#hold_validation_during_audit]
[src: file: backend/src/api/audit/docs_write.rs:57-96#validation_cancel_guard]

Human audit validation is deliberately different: it takes the lease first but
checks its completion gates before stopping validation agents. An unfinished
validator remains active and the validation request fails.
[src: file: backend/src/api/audit/validate.rs:315-494#validate_audit]

Accepted template commits must be integrated into the current checkout before
Full, Partial, or revalidation launches. The backend checks the merge branch
before launch and again while holding the lease. The UI also holds launches for
this state, including already audited projects; the optional-offer bypass cannot
skip accepted commits pending integration.
[src: file: backend/src/api/projects/template_merge.rs:324-330#audit_launch_refusal]
[src: file: backend/src/api/audit/full.rs:299-432#audit_launch_refusal]
[src: file: backend/src/api/audit/drift.rs:250-550#audit_launch_refusal]
[src: file: backend/src/api/audit/revalidate.rs:432-512#audit_launch_refusal]
[src: file: frontend/src/hooks/useTemplateMergeOffer.ts:28-93#useTemplateMergeOffer]

The regression suite exercises real HTTP refusals, cancellation before mutation,
late validation admission, pending integration for all three launch routes, and a
cancelled HTTP waiter whose blocking worker still owns the lease.
[src: file: backend/src/api/audit/docs_writer_tests.rs:1-204]
[src: file: backend/src/api/audit/docs_write.rs:103-151#docs_writer_lease_survives_http_cancellation_until_the_write_finishes]

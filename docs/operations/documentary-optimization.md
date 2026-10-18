# Documentary optimization budgets

The full audit runs a deterministic `documentary_optimization` phase after
reconciliation and before validation. It writes
`.kronn/audit/document-optimization.json`, Kronn's local working state (listed
in `.git/info/exclude`, never in your `.gitignore`); any blocking diagnostic prevents the
audit from completing and the validation endpoint re-runs the same gate before
recording validated state. [src: file: backend/src/api/audit/full.rs:3054-3121#analyze_and_write] [src: file: backend/src/core/document_optimization.rs:208-218#analyze_and_write] [src: file: backend/src/core/audit_state.rs:38-58#state_dir]
[src: file: backend/src/api/audit/validate.rs:610-626#blocking_diagnostics]

Defaults are 220 words per root adapter, 800 words for `docs/AGENTS.md`, 1,200
words for an agent's mandatory load path, and at most two initially routed
documents. Projects can override reviewed exceptions in
`docs/.kronn-document-budgets.json`; omitted fields retain their defaults.
Relaxing a default requires an `exception` object with non-empty
`justification`, `provenance`, `reviewed_by`, and `reviewed_at` fields. Hard
safety ceilings still apply: 800 adapter words, 1,200 `docs/AGENTS.md` words,
2,000 mandatory-path words, four initially routed documents, and a 2,000-word
large-inventory threshold.
[src: file: backend/src/core/document_optimization.rs:31-41#DocumentBudgets]
[src: file: backend/src/core/document_optimization.rs:259-296#validate_budgets]

The report includes bytes, words, estimated tokens, routed documents and the
five largest contributors for each detected adapter. [src: file: backend/src/core/document_optimization.rs:52-61#AgentLoadPath] [src: file: backend/src/core/document_optimization.rs:177-188#top_contributors]
Diagnostics cover
budget overruns, unresolved placeholders, broken local links and file
citations, duplicate or obsolete guides, orphan documents, large inventories
that are not search-first, and mutable citation line ranges; only broken links
and broken citations are blocking, the others are warnings. A citation may end
on the empty line N+1 of a newline-terminated file (what an editor numbers,
but never start there),
a parenthesised `(src: file: …)` is refused (only the bracketed marker is
verified), and every path or range the gate suggests is one it accepts. A Stack table
row without its `Latest stable` or `Support end` source and date (or
`not verified: no source`) raises the non-blocking `stack_currency` warning.
[src: file: backend/src/core/document_optimization.rs:1272-1287#inspect_stack_currency]
[src: file: backend/src/core/document_optimization.rs:1299-1371#stack_currency_problems]
[src: file: backend/src/core/document_optimization.rs:1373-1390#diag]

A citation into a path git ignores (a vendored dependency, a build output) that
is absent from the checkout is refused as before, but the message says the path
is git-ignored and cannot be verified from a fresh clone, instead of leaving a
bare "does not exist". [src: file: backend/src/core/document_optimization.rs:726-736#ignored_citation_message]

A tech-debt sheet is referenced from prose with a markdown link relative to the
document, or with the `:exists` presence form of a file citation; the rows Kronn
generates in `docs/inconsistencies-tech-debt.md` always use the link. The bare
form (the sheet's path with no line) is refused like any text file cited without
a line, and the step 8 prompt shows the accepted forms.

**Exact replacements are applied by Kronn** (KT-1353). When the gate's own
message carries the one replacement, the step's documents are corrected before
they are judged, the original is kept with the other citation originals, the
edit is logged and sent as a `documentary_autofix` event (`kind`, `from`, `to`
per fix). Applied: a link written from the repository root rewritten relative
to the document's folder (`docs/operations/` becomes `operations`); a bare
secret file (and a bare tech-debt sheet) given its `:exists` presence form; a
citation whose path resolves to exactly one other file that holds the cited
lines (the "did you mean" path); a range ending one line past the end of a file
with no closing newline, clamped to the last line. Never applied: a comma
bundle that is not fully verified, a comment inside a marker, several candidate
paths, a text file cited without a line, a range farther out of the file, and
anything inside fenced or inline code.
[src: file: backend/src/core/document_optimization/mechanical_fix.rs:36#mechanical_repair] [src: file: backend/src/api/audit/document_repair.rs:504#check_attempt]

**What the committed documents already got wrong is reported, not charged**
(KT-1353). A fresh Full run records the blockers its documents hold before step
1. As long as a blocker is unchanged (same code, document and cited subject) it
fails neither a step nor the run, even if a step rewrites the document around
it; it is listed in the final gate's `documentary_preexisting` event, stored on
the run, shown in the audit timeline and put at the top of the validation
prompt. The validation honours the exemption only while the document is exactly
as the run left it (content hash): edit the document and its problems block
again. See [audit run states](../architecture/audit-run-states.md).

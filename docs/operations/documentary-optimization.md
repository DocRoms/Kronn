# Documentary optimization budgets

The full audit runs a deterministic `documentary_optimization` phase after
reconciliation and before validation. It writes
`docs/.kronn-document-optimization.json`; any blocking diagnostic prevents the
audit from completing and the validation endpoint re-runs the same gate before
recording validated state. [src: file: backend/src/api/audit/full.rs:1715-1761]
[src: file: backend/src/api/audit/validate.rs:142]

Defaults are 220 words per root adapter, 800 words for `docs/AGENTS.md`, 1,200
words for an agent's mandatory load path, and at most two initially routed
documents. Projects can override reviewed exceptions in
`docs/.kronn-document-budgets.json`; omitted fields retain their defaults.
Relaxing a default requires an `exception` object with non-empty
`justification`, `provenance`, `reviewed_by`, and `reviewed_at` fields. Hard
safety ceilings still apply: 800 adapter words, 1,200 `docs/AGENTS.md` words,
2,000 mandatory-path words, four initially routed documents, and a 2,000-word
large-inventory threshold.
[src: file: backend/src/core/document_optimization.rs:12-38]

The report includes bytes, words, estimated tokens, routed documents and the
five largest contributors for each detected adapter. [src: file: backend/src/core/document_optimization.rs:33-72]
Blocking diagnostics cover
budget overruns, unresolved placeholders, broken local links and file
citations, duplicate or obsolete guides, orphan documents, large inventories
that are not search-first, and mutable citation line ranges.
[src: file: backend/src/core/document_optimization.rs:74-326]

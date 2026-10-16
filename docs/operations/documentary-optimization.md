# Documentary optimization budgets

The full audit runs a deterministic `documentary_optimization` phase after
reconciliation and before validation. It writes
`.kronn/audit/document-optimization.json`, Kronn's local working state (listed
in `.git/info/exclude`, never in your `.gitignore`); any blocking diagnostic prevents the
audit from completing and the validation endpoint re-runs the same gate before
recording validated state. [src: file: backend/src/api/audit/full.rs:2960-3027#analyze_and_write] [src: file: backend/src/core/document_optimization.rs:208-218#analyze_and_write] [src: file: backend/src/core/audit_state.rs:38-58#state_dir]
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
and broken citations are blocking, the others are warnings. A Stack table
row without its `Latest stable` or `Support end` source and date (or
`not verified: no source`) raises the non-blocking `stack_currency` warning.
[src: file: backend/src/core/document_optimization.rs:1087-1102#inspect_stack_currency]
[src: file: backend/src/core/document_optimization.rs:1121-1193#stack_currency_problems]
[src: file: backend/src/core/document_optimization.rs:1195-1212#diag]

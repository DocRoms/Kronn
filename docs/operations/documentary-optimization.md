# Documentary optimization budgets

The full audit runs a deterministic `documentary_optimization` phase after
reconciliation and before validation. It writes
`.kronn/audit/document-optimization.json`, Kronn's local working state (listed
in `.git/info/exclude`, never in your `.gitignore`); any blocking diagnostic prevents the
audit from completing and the validation endpoint re-runs the same gate before
recording validated state. [src: file: backend/src/api/audit/full.rs:3010-3077#analyze_and_write] [src: file: backend/src/core/document_optimization.rs:208-218#analyze_and_write] [src: file: backend/src/core/audit_state.rs:38-58#state_dir]
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
[src: file: backend/src/core/document_optimization.rs:1255-1270#inspect_stack_currency]
[src: file: backend/src/core/document_optimization.rs:1282-1354#stack_currency_problems]
[src: file: backend/src/core/document_optimization.rs:1356-1373#diag]

During an audit run (KT-1308), whatever the anti-hallucination mode, a
`[src: deps: …]` citation the injected dependency watch does not hold and a
`[src: url: …]` citation of a page the run's agent never fetched raise the
blocking `unbacked_provenance` diagnostic on the step that wrote them; its
feedback tells the model to write `not verified: no source` instead of a version
taken from memory. A page counts as fetched when the agent sent it to a fetch
tool (`WebFetch`, an MCP fetch), opened it in a browser (`browser_navigate`,
`navigate_page`, `new_page`), downloaded it with `curl` / `wget`, or was handed it
by a documentation tool (Context7: the URLs of the tool's result). Hosts compare
without a leading `www.`. The calls are read from the agent's tool calls (direct
stream-json, or the traces of an ACP-hosted agent). When the run used a channel
Kronn cannot observe — an agent that reports no tool inputs (an HTTP model), a
sub-agent (`Task`), or a Context7 result in an ACP trace, which never carries the
tool's output — `url:` citations are left unchecked, never blocked. Outside a run
(`analyze`) neither kind is checked. The pre-step baseline and the blame ledger
are computed with the same run-aware analysis (`analyze_with`), so an unbacked
citation that was already there does not block the step and one it wrote is
charged to it.

# Documentary optimization budgets

The full audit runs a deterministic `documentary_optimization` phase after
reconciliation and before validation. It writes
`.kronn/audit/document-optimization.json`, Kronn's local working state (listed
in `.git/info/exclude`, never in your `.gitignore`); any blocking diagnostic prevents the
audit from completing and the validation endpoint re-runs the same gate before
recording validated state. [src: file: backend/src/api/audit/full.rs:3145-3212#analyze_and_write] [src: file: backend/src/core/document_optimization.rs:222-232#analyze_and_write] [src: file: backend/src/core/audit_state.rs:38-58#state_dir]
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
[src: file: backend/src/core/document_optimization.rs:275-312#validate_budgets]

The report includes bytes, words, estimated tokens, routed documents and the
five largest contributors for each detected adapter. [src: file: backend/src/core/document_optimization.rs:58-67#AgentLoadPath] [src: file: backend/src/core/document_optimization.rs:191-202#top_contributors]
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
[src: file: backend/src/core/document_optimization.rs:1380-1395#inspect_stack_currency]
[src: file: backend/src/core/document_optimization.rs:1411-1483#stack_currency_problems]
[src: file: backend/src/core/document_optimization.rs:1481-1498#diag]

A citation into a path git ignores (a vendored dependency, a build output) that
is absent from the checkout is refused as before, but the message says the path
is git-ignored and cannot be verified from a fresh clone, instead of leaving a
bare "does not exist". [src: file: backend/src/core/document_optimization.rs:784-794#ignored_citation_message]

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
with no closing newline, clamped to the last line. Also applied (KT-1357, from
the 18/10 matrix): a list of lines of one file in one marker (`deploy.yml:210,214,218`)
split into one marker per line when every line is in range; a comment after a
`path:line` the gate accepts as written
(`docs/AGENTS.md:38-39: never edit generated files` inside the marker) moved out of the
marker, after it, in parentheses; a bare citation of an empty text file replaced
by the backticked path followed by `(empty)`. Never applied: a comma bundle that
is not fully verified (one line out of range, a path that is not there), a
comment after a reference the gate refuses, a comment holding a backtick, a
bracket or a parenthesis, a list with a comment, several candidate paths, a text
file cited without a line, an empty file cited with a line, a range farther out
of the file, and anything inside fenced or inline code.
[src: file: backend/src/core/document_optimization/mechanical_fix.rs:36#mechanical_repair] [src: file: backend/src/api/audit/document_repair.rs:545#check_attempt]

**Detector signals the agent never addressed** (KT-1357). Step 8 must dispose of
each deterministic detector signal: the index, or a TD sheet the index lists,
names the flagged file's path (any case), or the file's name followed by the
flagged line (`poc.html.twig:6`). The retry feedback now names each signal still
open with the exact text to write, and says a count ("all 12 signals"), a TD id or
a bare file name does not stand for it. When the last attempt still leaves some,
Kronn no longer fails the step and the run: it appends an
`## Unaddressed detector signals` section to the index (severity, dimension,
signal, evidence, one row each), replacing an earlier one, and sends a non-fatal
`warning` event. The section is the human's to triage; the step counts as done.
The same applies to the partial refresh of step 8, which also gives a malformed
coverage matrix back to the agent (three attempts) instead of failing on the
first.

**A refresh of an unchanged section runs no agent** (KT-1357). A partial refresh
asked for a section built from the whole source tree (repo map, glossary, TD
index, decisions, and the performance, accessibility, database, API and code
quality sub-audits) ends `unchanged`, with a
`step_unchanged` event and no agent, when the tree fingerprint is the one its
baseline recorded, no cited line is stale and its prose names no vanished path.
A revalidation always runs its agents. A step ending `unchanged` is recorded as
done (as in the Full pipeline), so a resume carries it.

**What the committed documents already got wrong is reported, not charged**
(KT-1353). A fresh Full run records the blockers its documents hold before step
1. As long as a blocker is unchanged (same code, document and cited subject) it
fails neither a step nor the run, even if a step rewrites the document around
it; it is listed in the final gate's `documentary_preexisting` event, stored on
the run, shown in the audit timeline and put at the top of the validation
prompt. The validation honours the exemption only while the document is exactly
as the run left it (content hash): edit the document and its problems block
again. See [audit run states](../architecture/audit-run-states.md).

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
stream-json, or the traces of an ACP-hosted agent, the default route). A trace
carries the URLs its call opened, read from the whole input before the bounded
excerpt cuts it, and the URLs a Context7 result named, read where the adapter
sees the result (the output itself is never copied); a native ACP tool is a fetch
by its `kind: "fetch"` as well as by its title. When the run used a channel Kronn
cannot observe — an agent that reports no tool inputs (an HTTP model), a
sub-agent (`Task`), or a documentation call whose result was not seen — `url:`
citations are left unchecked, never blocked; a result that was seen and named no
page (Context7's `resolve-library-id`) is not such a channel. A shell command
is a download only for a real `curl` / `wget` invocation (looking through `sudo`,
`env`, `xargs`, `;` / `&&` / `|` chains and `sh -c`), and only an `http(s)` argument
that is a destination of it counts (a positional argument, or `curl --url`). The rule
is conservative: a URL printed by `echo`, quoted, in a `#` comment or a heredoc body,
or given as the value of an option (`curl -d` / `--proxy` / `--referer` / `-o`,
`wget -O`) is text, not a fetch, and a command that is not fully understood (a
heredoc on it, a quote left open, a `curl` / `wget` option the reader does not
know, which may take a URL as its value) credits nothing, as does a display-and-exit
mode (`--version` / `-V`, `--help` / `-h`, `--manual` / `-M`), which transfers nothing.
The options of a wrapper (`sudo`, `env`, `xargs`, `timeout`, `nice`, `nohup`, `time`,
`command`, `exec`, `stdbuf`) are parsed with their values (`xargs -I curl printf …`
runs `printf`); a wrapper option not known credits nothing. The recorded URLs have one canonical
safe form, used on the stored and the cited side (`normalize_url`): no userinfo, a
credential-style query value (`sig`, `X-Amz-Signature`, `access_token`…) masked as
`name=***` after percent-decoding the name (`%73ig` is `sig`), vendor token shapes
redacted, non-secret parameters kept. A signed page still matches its citation; a
page with another plain parameter does not. Outside a run (`analyze`) neither
kind is checked, and only the full audit pipeline builds the run's provenance:
a partial audit checks neither. The pre-step baseline and the blame ledger
are computed with the same run-aware analysis (`analyze_with`), so an unbacked
citation that was already there does not block the step and one it wrote is
charged to it.

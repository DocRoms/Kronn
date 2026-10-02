# HTTP audit qualification — Gemini, 2026-10-02

Kronn completed a sixteen-step audit of a private Node service through LiteLLM
Economy using `gemini-3.6-flash`, after recorded recoveries. Its initial HTTP
validation turn also completed. This establishes execution and recovery for this
configuration; it does not establish uninterrupted reliability, the accuracy of
every finding, or compatibility with every HTTP/Ollama model. The private source
and generated documents remain in the local benchmark.

## Observed runs

Each recovery used an isolated copy of the preceding output and a frozen runtime.
No generated output was repaired by hand. Earlier runs and their failure evidence
were retained.

| Run | Runtime | Outcome and cause |
|---|---|---|
| HG2 | `9bdb44d5` | 15/16; the write budget prevented completing an index, and documentary blockers remained. |
| HG3 | `adf442fa` | DNS loss during step 6 interrupted the fresh run; later launches also failed. |
| HG3R1 | `e5ac691e` | 15/16; documentary recovery succeeded, but provider HTTP 429 interrupted step 8 after partial writes. |
| HG3R2 | `b5093b81` | 15/16; no provider failure, but a coverage-table row lacked its third cell and correctly failed validation. |
| HG3R3 | `043c6444` | 16/16; the remaining step completed with the recomputed coverage diagnostic. |

These observations led to bounded per-tool audit budgets and documentary repair
(KT-951/952), safe persisted provider-failure diagnostics (KT-955), and targeted
coverage feedback sharing the existing correction budget (KT-956). Provider
errors do not trigger the documentary correction loop or replay completed tool
effects. Invalid output still fails after the correction budget is exhausted.
[src: commit: adf442fa] [src: commit: e5ac691e]
[src: commit: b5093b81] [src: commit: 043c6444]

## Final recovery and remaining quality limits

HG3R3 ran from 00:41:07 to 00:41:50 UTC on 2026-10-02. Only step 8 executed
again, taking 40.807 seconds; fifteen successful rows were inherited. The initial
validation turn then completed in 79.217 seconds on the same model. Its ten
Critical/High debt cards await human choices: the harness did not answer them or
assign a human-validated badge.

The final output contains 87 Markdown files and 49 debt files, with zero blocking
documentary diagnostics and 848/848 concrete file references resolving to existing
paths and line ranges. The central index mentions all 49 debt identifiers but
does not link them; KT-949 remains a follow-up. The final recovery changed only
the existing central index; validation added a reconciliation document. No
application source changed. Bounded checks found no reproduction of the two known
fixture credentials or test canaries; this is not an exhaustive secrecy proof.

A semantic spot check found references to existing but unrelated lines and a
false positive declaring dependencies missing even though a subproject manifest
declares them. The validation agent repeated that false positive while claiming
factual alignment. Citation existence and a successful validation *execution*
therefore do not certify factual correctness. This spot check is not another
blind judgment or a complete scoring of the 23 ground-truth facts. See the
separate [Sonnet comparison](sonnet-template-comparison-2026-10-01.md) for the two
independent Codex judgments.

Reported HG3R3 audit usage was 295,853 input plus 6,717 output tokens. The 195,101
cache-read tokens are already included in input. Inherited steps have unknown
new usage, not measured zero consumption. The initial validation turn separately
reported 664,631 tokens. Recovery usage is not the cost of a fresh full audit.

## Evidence and automated checks

The local `kronn-ab-bench/QUALIFICATION-http-gemini-20261001.md` records the full
sequence, metrics, runtime hashes, test logs and limitations. Each
`http-gemini-20261002-hg3*` directory retains SSE, persisted results, documentary
diagnostics and source/output hashes. Owned test backends stopped normally after
their final validation turns; the user's backend was not stopped.

HG3R3 run id: `57dbb375-0fdd-46f0-bbec-cb0949f576f6`.
Its `evidence-frozen.json` identifies 120 artifacts; its own SHA-256 is
`07a230e40f5d0be8dcd6a8e97b32e230dac10aafda52b1ec65da4a42133f5ba8`.
This identifies private evidence, not a publicly reproducible fixture.

Runtime `043c6444` passed 372 audit tests, format, Clippy on all targets and a
fresh build. The regression suite exercises successful correction, failure after
the attempt limit, preservation of human content and partial files, cumulative
usage, and no corrective retry after provider 429.
[src: commit: 043c6444]

Follow-up `90b844a` fixes the fixture's command-wrapper lint failure and separates
detached validation calls from its audit-attempt counter. The latter could count
a fourth request even though the audit correctly stopped after three attempts.
The corrected regression, formatting and command lint pass; production behavior
and the frozen benchmark runtime are unchanged.
[src: commit: 90b844a]

The [complete CI for `b5093b81`](https://github.com/DocRoms/Kronn/actions/runs/36944299010)
passed all sixteen jobs and DCO. A prior local full command on that revision
failed while an existing Git fixture created an empty commit; isolated and
module-level reruns passed, but its cause is unconfirmed. `a0763b66` adds the
missing failure diagnostics; it is not claimed to fix that failure. Current
release qualification must use the final integrated revision rather than
substituting these earlier results.
[src: commit: a0763b66]

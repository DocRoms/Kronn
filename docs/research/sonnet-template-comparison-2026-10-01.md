# Sonnet template comparison — independent review, 2026-10-01

One audit of the same private Node service was run with Claude Code / Sonnet
on 0.14.1 (`6fdc602`) and 0.14.2 (`09faf2e2`). Two fresh Codex sessions then
evaluated the frozen outputs independently, with anonymous labels and reversed
reading order. The user explicitly selected two Codex judges; they are separate
contexts, not separate model families. No global score was computed.
[src: commit: 6fdc602] [src: commit: 09faf2e2]

Each judge inspected all 23 ground-truth facts, 15 defect criteria, every debt
file, and the same 33 predetermined citation samples. Both judgments were
frozen before confrontation. The package masks credentials and preserves line
numbers. Detailed project findings remain in the local benchmark, rather than
publishing private source or generated project documentation.

## Results and limits

| Measure | 0.14.1 | 0.14.2 |
|---|---:|---:|
| Successful audit steps | 16/16 | 16/16 |
| Pipeline outcome | Interrupted by documentary check | Completed |
| Validation discussion created | No | Yes |
| Duration | 888.804 s | 802.350 s |
| Recorded input + output, excluding Anthropic cache | Unknown | 98,542 |
| Debt files assessed | 35 | 37 |
| Known defects found, both judges | 13/15 | 13/15 |
| Certain defects found, both judges | 10/11 | 10/11 |
| Fully correct facts, judges 1 / 2 | 13 / 12 of 23 | 11 / 11 of 23 |
| Actionability at least 3/4, judges 1 / 2 | 30 / 34 of 35 | 28 / 36 of 37 |
| Concrete file citations with valid full ranges | 317/320 | 295/295 |
| Central index direct links to debt files | 7/35 | 6/37 |

Supplementary measurements on 2026-10-02 do not change the frozen judgments:
all 35 older TDs and all 37 newer TDs have a link from at least one general or
specialized index. Direct central-index coverage alone therefore understates
discoverability. A small domain router is sufficient; agents need not load
every linked document.

The newer run's persisted counters are 262 uncached input tokens, 98,280 output
tokens, 4,529,551 cache-read input tokens and 525,743 cache-write input tokens.
Anthropic reports these input categories separately: the cumulative total is
5,153,836 tokens including cache, not 98,542. This is cumulative usage across
calls, not one context window or a monetary-cost estimate. Future comparisons
must retain the separate categories and include retries and validation usage.
The original older run still has missing telemetry.
[src: file: backend/src/agents/runner_test.rs:811]

Both outputs miss the same certain credential finding. Each also misses one
different uncertain defect. Both contain material factual errors and stale
index summaries. The newer output includes one duplicate core finding. The
judges differ on four of 46 fact classifications and on how unsupported
reasoning affects otherwise actionable remediation. Both score series are
retained; averaging them would conceal those differences.

The operational completion improvement is observed. Overall content
non-inferiority or superiority is **not established**. There is one sample per
arm, template-family hints weaken blinding, and backend and templates changed
together. The later template fixes at `19d27c6b` were not exercised by this pair.
The [HTTP/Gemini qualification](http-audit-qualification-2026-10-02.md) on later
revisions is a separate experiment.
[src: commit: 19d27c6b]

The original keyword-based recall underestimated both outputs. Mechanical
citation checks also missed two range endings beyond EOF in the older output.
The corrected denominator excludes six copied specification examples and two
grammar metavariables per arm. Path/range validity does not establish semantic
support: the judges document weak anchors and overbroad claims separately.
Their semantic samples are not comparable random samples and include different
document categories.

Neither output changes tracked application source or reproduces the two test
canaries. Both modify `.gitignore` through generated tooling exclusions. The
older run's zero in legacy telemetry means unknown cost, not zero consumption.
The frozen ground truth itself contains stale line references and an incorrect
severity sum. Its suggested GitHub Actions casing defect was also rejected:
string comparisons ignore case; an independent mutable-ref criterion still
satisfies the same defect row for both outputs.
[src: url: https://docs.github.com/en/actions/reference/workflows-and-actions/expressions#operators]

## Evidence retained locally

The benchmark directory `kronn-ab-bench` contains the original AS1/CS1 outputs,
the frozen rubric and truth, `COMPARATIF-sonnet-codex-20261001.md`, and
`judgment-20261001-codex/`. The original comparison and metrics are preserved
as provenance, superseded by this independent review and corrected inventory.

Frozen score-file SHA-256 values:

- Judge 1: `be89d78632a7903df20997b7ec72d8376b7351af44fd5123b00225ca9ab0a5e0`
- Judge 2: `dde6882f4534286caad5495e68572700a30452eca534e6cd1ad9e3004c130bc2`

The control manifest records output/source hashes, anonymization mapping and
the freeze receipts. These hashes identify private evidence; they do not make
the experiment independently reproducible from this public repository alone.

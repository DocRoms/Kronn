# Token Economy — 0.9.6 release documentation

What 0.9.6 changed about Kronn's token cost, what each figure proves, and what it
does not. Companion to the measurement contract in
[`../design/token-economics-baseline.md`](../design/token-economics-baseline.md),
which defines the KPI and the collector.

## 1. Three costs, never one number

Every figure in this release belongs to exactly one of three quantities. They are
not interchangeable, and no surface in Kronn adds them together.

| Quantity | What it is | What it is for |
|---|---|---|
| **Raw traffic** | non-cached input + cache write + cache read + output | context-window dependence and quota pressure |
| **Cache reads** | the share of raw traffic served from a prompt cache | why raw traffic is large and cheap at the same time |
| **Billable cost** | raw traffic minus cache reads, priced per tier | what appears on an invoice |

Measured on one real Claude Code session on this machine: **cache reads were 98.4 %
of raw traffic**, and billable tokens were smaller than raw traffic by a factor of
about **62×**. A report that quotes raw traffic as "cost" overstates spend by that
factor; one that quotes billable tokens as "context pressure" understates the
window problem by the same amount. Both readings have happened, which is why the
distinction is enforced in code and not only in prose.

**Billable is `None`, not 0, when it cannot be derived.** It requires cache reads
from *every* measured session in the aggregate. Mixing a vendor that splits caches
(Claude Code) with one that does not (Vibe) makes the difference underivable, and
summing what is available would understate the cache share instead of admitting the
gap. See `db::cli_telemetry::ObjectSpend::billable_tokens`.

## 2. What an absent measurement means

The rule that governs every counter added in this release:

> **An unmeasured cost is unknown, never zero.**

On one real session, **4 308 007 075 tokens of traffic were stored as `0`** because
Kronn had no counter for a CLI it did not spawn. That single reading is why:

- `session_tokens_at_message` is `NULL` when unmeasured, never `0`;
- the discussion header shows "unknown" for an unmeasured CLI, dimmed but **not
  hidden** — a joined CLI whose spend is unaccounted for must stay visible;
- `TelemetryCoveragePanel` renders coverage and deliberately no total;
- an agent-context total in the audit is `None` when any one file is unreadable,
  because a partial sum presented as a total is wrong by an unknown amount;
- `compactTokens` renders a small real cost as `<1k` rather than rounding to `0`.

## 3. What shipped

**Bounded what was unbounded.** Three leaks were found by measuring rather than by
reasoning: a debate context that sent **1 320 210 B** to a model, `disc_load_other`
returning whole discussions, and CLI sessions with no ceiling at all. Each now has
a cap, and every cap announces its own truncation.

**Replaced agent passes with mechanical ones.** Quick Exec runs a deterministic
command and returns a bounded summary (cap **4 096 B**, compile-time asserted) with
the full streams kept on disk. A review is now a delta against a ledger keyed to
the cause rather than to the comment, so a re-review costs what changed.

**Made adoption and context cost visible.** `docs/AGENTS.md` went from **84 224 B to
13 471 B** behind a ratchet that tightens on every gain; the Context Architecture
Audit reports what each agent loads in any monitored project; `/api/rtk/state`
surfaces RTK adoption — **37 %** by session, and names both sources that currently
cannot answer.

## 4. What the benchmarks prove, and what they do not

**Review deltas** (`backend/scripts/rereview_benchmark.py`, measured on discussions
`095dfee0…` and `8490d400…`: 723 messages, 162 review passes):

| Costing | Per pass | vs a cold pass | vs a warm pass |
|---|---|---|---|
| measured p90 payload | 564 B | −99.8 % | −94.4 % |
| measured worst payload | 2 199 B | −99.1 % | −78.3 % |
| enforced ceiling | 24 576 B | −90.3 % | **+142.6 %** |

Read the last row. Costing every pass at the ceiling makes a bounded payload look
2.4× *more* expensive than a warm session's incremental context. The ceiling is a
backstop for a pathological diff; the p90 and worst rows are what the shipped
renderer produces, pinned by two tests so growth fails a build.

**Two before-figures, because the answer depends on which applies.** A *cold* pass —
a returning agent, a new session, a `disc_load_other` — pays for everything in the
discussion so far. A *warm* pass pays only for what arrived since. The reference
discussions show the cold pattern; a live session is warm. Quoting only the cold
figure would flatter this release.

**RTK compression is intact and not improved by us.** `rtk` is an external binary;
0.9.6 changes none of it. `backend/scripts/rtk_residual_benchmark.py` pins the
test/lint rates as floors — vitest 96.6 %, cargo test 94.7–100 %, eslint 99.9 % —
so any later change aimed at the residual cannot quietly trade them away.

**The residual is quantified, not reduced.** Measured over 2 901 Bash calls: 17 %
adoption, 2 172 missed invocations, ~1.37 MB returned unfiltered, and the single
largest item is `rtk read` at **10.6 % over 985 calls**. Reducing it needs changes
inside RTK (tracked as KT-256), so **this release does not claim a shell-residual
reduction.**

**Controlled A/B/C/D replay.** `backend/scripts/token_economics_ab.py` gives Claude
Code and Codex the same four-field engineering case through the four release-gate
strategies. The long-session fixture is 180 kB — conservative against the 452,876 B
reference room — while C and D use the bounded resume bundle, Quick Exec proof and
review-ledger delta. Five repetitions per provider and variant produced **40/40
exact answers**, so success and the deterministic quality score did not regress.

| Provider | A median raw traffic | D median raw traffic | reduction | A P90 duration | D P90 duration |
|---|---:|---:|---:|---:|---:|
| Claude Code | 75,880 tokens | 10,506 tokens | **−86.15 %** | 8,425 ms | 7,624 ms |
| Codex | 50,254 tokens | 13,635 tokens | **−72.87 %** | 7,749 ms | 5,583 ms |

The complete aggregate and per-run counters are committed in
`docs/research/token-economics-ab-2026-08-12.json`; prompt and answer bodies are
not exported. This proves the benefit for one controlled, deterministic task
class. It does **not** claim that every real workflow saves the same percentage.
The separate first-turn MCP smoke asked both CLIs to invoke `bridge_info` against
the current bridge: both called the tool and returned its real `stale: false`
field, demonstrating that the reduced catalogue still supports first-turn tool
discovery and execution.

External telemetry coverage has two defensible readings — 100 % of *measurable*
conversations, 7 % of *all* sessions — and the distinction remains explicit rather
than being collapsed into a flattering single percentage.

## 5. Rollback

Everything in this release is additive: new tables, new endpoints, new caps. No
existing column changed meaning, and no data is rewritten.

| To undo | Do this | Consequence |
|---|---|---|
| a cap that is too tight | raise the constant and re-run its gate | the gate's ceiling is pinned to the current size, so a raise is a visible, reviewed act |
| Quick Exec | stop calling `/api/quick-exec`; `core::quick_exec` has no background task | recorded runs stay as history |
| the review ledger | stop calling it | findings remain queryable; nothing else reads the tables |
| CLI telemetry | stop the collector | counters go `NULL`, which reads as *unknown* rather than as free — by design |
| the context audit | stop calling the endpoint | the module has no write path, so there is nothing to revert on disk |

Migrations 113–120 are additive (`ALTER TABLE … ADD COLUMN`, `CREATE TABLE IF NOT
EXISTS`). During development, migrations now finalized as 113–119 briefly shipped
under identifiers 107–113 before the branch was rebased onto 0.9.5. Startup maps
those historical receipts to their final identifiers without replaying SQL.
Downgrading the binary leaves the extra columns unread; the older build does not
fail on them. Restoring a pre-0.9.6 database is only needed if a column must be
*removed*, which no rollback above requires.

**One thing rollback cannot restore:** a session whose telemetry was never collected
has no counters to recover later — the vendor transcript is the only source, and it
rotates. Coverage gaps are permanent, which is the argument for running the
collector rather than for keeping it optional.

## 6. Where the numbers live

- `backend/scripts/token_economics.py` — the KPI collector and its baseline snapshots
- `backend/scripts/rereview_benchmark.py` — review-pass input, cold and warm
- `backend/scripts/token_economics_ab.py` — controlled A/B/C/D replay with native
  Claude/Codex usage counters, duration and deterministic quality scoring
- `backend/scripts/rtk_residual_benchmark.py` — compression floors and residual ranking
- `backend/scripts/ci/context_budget.py` — the instruction-file ratchet
- `backend/scripts/ci/mcp_surface_budget.py` — the MCP surface ratchet
- `backend/scripts/rtk_bypass_audit.py` — commands that bypassed RTK, probes excluded

Bytes are exact and gate things. Token counts are estimates at 3.7 B/token, printed
and never used as a threshold: a real tokenizer is model-specific, and a ceiling
must not rest on an approximation.

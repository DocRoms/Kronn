# Token Economics — baseline contract (KT-188, 0.9.3)

Canonical metric, measurement schema and reproduction protocol for the
token-reduction campaign (KT-187). Every later claim of the form "this
change saved N tokens" must be measured against this contract; anything
else is an anecdote.

Collector: `backend/scripts/token_economics.py` ·
Tests: `backend/scripts/test_token_economics.py` (`make test-python`) ·
Baseline snapshots: `docs/research/token-economics-baseline-*.json`.

## 1. The KPI

**Raw context traffic per completed task, at comparable quality.**

`--completed-tasks N` records the denominator explicitly. The report exposes
`raw_traffic_tokens_per_completed_task_by_agent` and deliberately does not
invent a cross-provider total when one provider is absent. Without a positive
denominator the normalized KPI is `null` and the report declares a data gap;
raw totals remain available but must not be presented as the headline KPI.

Raw traffic = non-cached input + cache write + cache read + output, i.e.
the size of context transported and reprocessed per model call. It is a
measure of context-window dependence and quota pressure, **not** of
billing: cache reads dominate it (~98 % for Claude on this machine) and
are billed at a fraction of uncached input. The two must never be
conflated, in any report or dashboard.

## 2. Measurement schema

Every measurement separates, per provider/agent:

| Field | Meaning |
|---|---|
| `non_cached_input_tokens` | input actually processed uncached |
| `cache_write_tokens` | cache creation (Claude `cache_creation_input_tokens`, Copilot `cache_write_tokens`; Codex does not expose it → `null`) |
| `cache_read_tokens` | input served from provider cache |
| `output_tokens` | generated tokens |
| `reasoning_tokens` | reasoning subset of output where exposed (Codex, Copilot) |
| `raw_traffic_tokens` | sum of the above input+output components available for that provider |
| `estimated_cost_usd` | billing estimate — `null` until a per-model tariff table is configured; never derived from raw traffic |

Rules:

- **`null` and `0` are never interchangeable.** Every null has an exact path
  in `null_reasons` and one typed reason: source unavailable, unsupported
  metric, undefined ratio, unconfigured cost, insufficient granularity,
  missing denominator, not requested, unavailable probe, no observed event or
  an omitted non-canonical value. `data_gaps` separately records source and
  measurement limitations that affect interpretation; an undefined `0/0`
  ratio and an intentionally unconfigured tariff are not falsely described as
  missing telemetry. A readable, sufficiently granular source with no activity
  reports measured zeros. The validator requires every identity, counter,
  count, cost, coverage, provenance, RTK installation/version field and the
  exact null-reason path set; wrong types, missing fields and extra export
  fields fail closed. Human-readable `data_gaps` are mirrored byte-for-byte by
  `data_gap_details` (`id`, source, typed code, message). Every null classified
  as `source_unavailable` or `insufficient_granularity` must reference a
  matching structured gap through `null_gap_links`; missing, dangling or
  source-mismatched links invalidate the report.
- A JSONL file that cannot be read makes its whole Claude or Codex source
  unavailable instead of silently producing measured zero. Invalid JSON lines
  produce a source-quality gap only when they can be deterministically assigned
  to the requested window: Claude requires a recoverable timestamp on the raw
  line, while Codex may also use a path day wholly contained in the window.
  Unplaceable Claude corruption is deliberately excluded from fixed-window
  quality accounting because later appends must not mutate historical reports;
  no file modification time or wall clock is used as a surrogate timestamp.
  Codex path days wholly after the window are not opened. Positive traffic
  also requires positive provider activity counts and non-zero top-session
  shares; the validator rejects impossible session/call/rollout counts and
  enforces exact `100%` top-session ratios when every measured session is
  necessarily inside the reported top set. Claude's repository share is
  `not_requested` when no repository filter was supplied, while a requested
  filter over zero traffic remains an `undefined_ratio`.
- Every source carries `provenance` (what was read and how it was
  deduplicated) and `coverage` (the observed timestamp range **inside the
  requested window**). Timestamped events before or after the window never
  enter exported coverage. A source with no event in the window has null
  coverage with a `no_observed_events` or `source_unavailable` reason.
- Identifiers leaving the tool are **pseudonymized**, not anonymized. The
  `repo_pseudonym` is an unsalted SHA-256 prefix: it removes the raw label from
  the export but remains linkable and may be guessable from a small candidate
  set. Treat the report as internal metadata. Free-form scenario labels are
  never exported; the CLI accepts only the four canonical labels below.
  Telemetry records are parsed in memory (which necessarily includes
  their content fields), but conversation/prompt content, file bodies
  and secrets are never stored, aggregated or exported — only counters,
  timestamps, models and tool **names** (to attribute
  `disc_wait_for_peer` turns) reach the report.

Local sources and their dedup rules:

- **Claude Code** — `~/.claude/projects/**/*.jsonl`. Streaming can write the
  same `(requestId, message.id)` several times, so duplicates are canonicalized
  using the earliest timestamp, the largest complete usage snapshot (one
  coherent vector, never field-wise synthetic maxima), and the union of tool
  names across snapshots.
- **Codex** — `~/.codex/sessions/**/rollout-*.jsonl`, `token_count`
  events are *cumulative*, and a forked/resumed rollout **replays its
  parent's counters**. The window contribution of a thread is therefore a
  delta, never its lifetime total. Rollouts are grouped by
  `session_meta.session_id`, but a fork boundary may pair only with its own
  earlier snapshot or a timestamped root-thread ancestor. Component counters
  and the derived non-cached cumulative counter must all be monotonic; a
  divergent or unproven branch makes Codex metrics null with an explicit gap
  instead of synthesizing an impossible negative non-cached delta. Among
  coherent branches, the largest thread delta is retained to avoid replay
  double-counting. A replay-only fork has zero own delta and is not counted as
  an active rollout. A root session known to predate the window also requires
  a pre-window cumulative counter boundary — its first observed in-window
  lifetime snapshot is never subtracted from zero, including when the old
  session metadata lives in another rollout file. Timestamp-less events may
  use their path date only when that complete UTC day is contained in the
  window or lies wholly before it as a boundary snapshot; partial boundary
  days are always omitted and declared, so appending
  a timestamp-less event later on the same boundary cannot mutate the
  historical measurement. Codex does not expose cache-write usage, so the required
  `cache_write_tokens` identity is always null with `unsupported_metric`.
  Measured impact of root grouping on 2026-08-02: 16.8 G (naive per-file) →
  4.7 G (per-thread) over 30 days.
- **RTK** — `rtk gain --daily --format json`; RTK exposes whole UTC days, not
  event timestamps. Only complete days contained in the requested window are
  summed. Partial boundary days are omitted and declared in `data_gaps`; a
  same-day scenario therefore reports RTK counters as `null`, never as a
  misleading whole-day allocation. `rtk_installed` reflects successful
  executable invocation and does not depend on the optional version probe.
- **Copilot** — `~/.copilot/session-store.db`,
  `assistant_usage_events`, read-only, aggregate SQL only. SQLite timestamps
  are normalized with `julianday`, so both `YYYY-MM-DD HH:MM:SS` and ISO
  `T...Z` rows obey the same inclusive boundaries. The report states whether
  the source's observed range spans the requested window; sparse history is a
  declared coverage gap, not silent completeness.
- **Kronn** — `kronn.db` read-only: telemetry honesty (share of external
  agent replies whose `tokens_used` is actually populated). The validator
  recomputes that percentage from total replies minus the per-agent untraced
  reply counts; a rounded value that does not match exactly is rejected.

## 3. The four canonical scenarios

A scenario run is one explicit measurement window: note the wall-clock
start/end of the exercise, then measure with `--from/--to --scenario`.
The task performed must be the same class of work across scenarios
(same repo, comparable KT-sized change) for the comparison to hold.

| Label | Protocol |
|---|---|
| `native-agent` | The task is done by a Kronn native discussion agent only. No CLI joined. |
| `cli-oneshot` | A fresh CLI session is launched for the task and exits when done. No `disc_wait_for_peer`. |
| `cli-persistent` | An existing long-lived CLI session does the task (joined room, no waiting loop during the window). |
| `cli-persistent-wait` | Same as above but the session holds the room with the `disc_wait_for_peer` loop during the window. |

Reproduction command (identical for all four, only the label and window
change):

```sh
python3 backend/scripts/token_economics.py report \
  --from <ISO start> --to <ISO end> \
  --scenario cli-persistent-wait \
  --completed-tasks 1 \
  --repo-alias Kronn --repo-filter Kronn \
  --json out.json
```

The report marks whether the label is one of the four canonical ones
(`scenario_is_canonical`), so exploratory labels stay distinguishable.

## 4. Baseline snapshots

A baseline is the team JSON produced by the collector over a stated
window, committed under `docs/research/` with the date in the filename.
It contains provenance and coverage per source and is pseudonymized by
construction (hashed repo label, no session content, machine paths only
as generic provenance descriptions). Team members can produce the same
JSON with the mini-prompt in the token-economics audit discussion and
results are comparable field by field.

Comparison rules (from the audit, unchanged):

- normalize per active day / agent call / session;
- compare median and P90, not only the mean;
- segment "with Kronn" / "RTK only" / "without Kronn";
- headline KPI: raw traffic per completed task;
- a Kronn-assisted workflow that consumes more than its non-Kronn
  equivalent at comparable quality is a critical regression.

## 5. Determinism and testing

`build_report` is a pure function of its inputs. By default `generated_at` is
the explicit window end, so identical collector outputs produce byte-identical
JSON without a wall-clock dependency. Coverage is also window-scoped, and the
Codex path-date fallback is disabled on partial boundary days, so later
timestamped events and ambiguous boundary events cannot alter a regenerated
snapshot.
The committed baseline is loaded by the test suite and checked against the
strict current schema, typed null contract, normalized KPI arithmetic and
per-provider raw-traffic identities.
Each collector is tested against synthetic fixtures whose expected
values are hand-computed, including the dedup rules, the window filter,
the null-vs-zero contract and the pseudonymization of `repo_pseudonym`.
Privacy canaries cover Claude text, tool inputs, working directories, session
and request identifiers plus Codex content; none may reach serialized output.

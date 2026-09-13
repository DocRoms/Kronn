# `messages.cost_usd` is not exclusively a real measurement

A non-null `messages.cost_usd` looks like a genuine, provider-reported cost,
but only Claude Code's own CLI ever reports one: its `stream-json` `result`
line carries a real `cost_usd` field, captured as `stream_json_cost`.
[src: file: backend/src/agents/runner.rs:9444]
[src: file: backend/src/agents/runner.rs:9535-9539]
[src: file: backend/src/api/discussions/streaming.rs:3020-3030]

Every other agent type never emits that field (only the Claude Code launch
path sets `OutputMode::StreamJson`), so when `stream_json_cost` is `None` the
same ingest path falls back to `pricing::estimate_cost(agent_type,
tokens_used)` and writes *that* estimate straight into `messages.cost_usd`.
[src: file: backend/src/agents/runner.rs:8540-8548]
[src: file: backend/src/api/discussions/streaming.rs:3463-3476]

So a query that treats "`cost_usd` is non-null" as "this is a measured cost"
is wrong for every agent except `ClaudeCode` — it silently upgrades a
previously-computed estimate to look like an exact measurement. There is no
column that distinguishes the two cases; the distinction has to be
reconstructed from `agent_type` at read time.

`CostAggregate::add` (KT-637) does this reconstruction: a non-null
`cost_usd` only counts as "measured" (not flagged `has_estimate`) when
`agent_type == "ClaudeCode"`; for every other agent a non-null `cost_usd` is
folded into `known_usd` but still marked `has_estimate = true`.
[src: file: backend/src/models/stats.rs:32-58]

If a future agent adapter starts reporting its own real cost (a second
`OutputMode::StreamJson`-like path), this reconstruction must be revisited —
otherwise its genuine measurement will be mislabeled as an estimate.

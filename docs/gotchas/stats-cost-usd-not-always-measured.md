# `messages.cost_usd` is never provably "measured", for any agent

A non-null `messages.cost_usd` looks like a genuine, provider-reported cost,
but the ingest path computes it from the reply's detailed counters and the
serving model's rate for **every** agent, including `ClaudeCode`, whenever a
real cost isn't available: `pricing::price_reply(..., stream_json_cost, counters)`
(KT-894 — it used to be `estimate_cost(agent_type, tokens_used)`, a 60/40 split
of the total that billed cache reads as fresh input). With no counters or no
confirmed rate the cost is NULL (unknown), and `message_usage.cost_unknown_reason`
says why. Rows written before KT-894 keep their old figure, and nothing marks
which ones those are.
[src: file: backend/src/core/pricing.rs:340-363#price_reply]
[src: file: backend/src/api/discussions/streaming.rs:5429-5435#price_reply]
[src: file: backend/src/db/message_usage.rs:17-24#MessageUsage]
[src: file: backend/src/db/message_usage.rs:26-61#record]

`stream_json_cost` is only ever populated from Claude Code's own `stream-json`
`result` line, so only ClaudeCode messages can *possibly* carry a real
measurement — but there is no column recording whether a given `ClaudeCode`
row actually got one, or fell back to the same estimate as everyone else.
[src: file: backend/src/agents/runner.rs:14095-14148#total_cost_usd]
[src: file: backend/src/api/discussions/streaming.rs:4840-4866#stream_json_cost]
[src: file: backend/src/db/message_usage.rs:38-48#message_usage]

So a non-null `cost_usd`, for ANY agent_type, is data whose provenance is
unguaranteed: it might be a real measurement, or it might be an old
computed estimate. `CostAggregate` (KT-637) never asserts either
direction from `agent_type` alone. It folds a non-null `cost_usd` into
`recorded_usd` / `has_recorded` — "known, provenance unguaranteed" — and for
tokens that have **no** persisted cost it has only a token total to work with,
which cannot be priced: they land in `unknown_cost_tokens`, except local
inference (Ollama), which is an `estimated_usd` of zero.
[src: file: backend/src/models/stats.rs:7-44#CostAggregate]
[src: file: backend/src/models/stats.rs:54-94#add]

Because `SUM(cost_usd)` in SQL silently drops NULL rows, a `GROUP BY`
aggregate cannot just read `SUM(tokens_used)` and `SUM(cost_usd)` for a group
— that makes every token in the group look priced, even the ones whose row
was NULL. Every `messages` query in `token_usage` that feeds
`CostAggregate::add` also computes
`SUM(CASE WHEN cost_usd IS NOT NULL THEN tokens_used ELSE 0 END)` per group,
so `CostAggregate::add` receives the exact token count that had no recorded
cost and can price (or mark unknown) only that share. `agent_usage` reads no
cost at all.
[src: file: backend/src/api/stats.rs:6-20#DiscRow]
[src: file: backend/src/api/stats.rs:63-74#disc_stmt]
[src: file: backend/src/api/stats.rs:161-170#top_disc_stmt]
[src: file: backend/src/api/stats.rs:231-241#daily_stmt]

If a future agent adapter starts reporting a cost through a dedicated,
distinguishable path (e.g. a `cost_source` column), this reconstruction can
be relaxed for that path specifically — but as long as `messages` has no
such column, no query may treat "`cost_usd` is non-null" as "this is a real
measurement" for any agent, ClaudeCode included.

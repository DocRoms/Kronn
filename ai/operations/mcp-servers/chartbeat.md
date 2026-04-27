# Chartbeat — Usage Context

> Instructions for agents calling the **Chartbeat** API via curl.

Chartbeat has **two API families with DIFFERENT auth mechanisms and
call patterns**. Using the wrong auth on the wrong endpoint is the #1
source of 401/404 confusion — read §1 and §2 before composing anything.

## 1. Live Publishing API — `apikey=` query param, synchronous

All `/live/...` endpoints. Direct GET, JSON response.

```bash
curl -s "https://api.chartbeat.com/live/quickstats/v4?apikey=<KEY>&host=example.com"
```

`host` selects the tracked site. For multi-locale sites pass the
concrete sub-domain (`host=de.example.com` for a German edition). The
plugin config stores a default `host` but agents SHOULD override per
question when the user mentions another edition.

## 2. Historical / Query API — `X-CB-AK` HEADER, ASYNCHRONOUS

**Critical**: historical endpoints do NOT accept `apikey` as a query
parameter. They require the header `X-CB-AK: <KEY>`. Passing the key
in the URL on `/historical/...` or `/query/...` returns 401 or 404
that LOOK like access errors — it's just the wrong auth channel.

Two endpoint shapes observed in the wild, both asynchronous:

- **Modern**: `/query/v2/submit/page/` then `/query/v2/status/?query_id=<id>` then `/query/v2/fetch/?query_id=<id>`
- **Legacy**: `/historical/traffic/series/` (accepts the header directly, returns data synchronously in most cases — but still OK to retry with the modern flow if it doesn't)

### The 3-step async flow (modern, `/query/v2/...`)

```bash
# 1. Submit — returns { "query_id": "..." }
curl -s "https://api.chartbeat.com/query/v2/submit/page/?host=example.com&start=2026-04-13&end=2026-04-20" \
     -H "X-CB-AK: <KEY>"

# 2. Poll status — { "status": "running" | "completed" | "failed" }
curl -s "https://api.chartbeat.com/query/v2/status/?query_id=<QID>" -H "X-CB-AK: <KEY>"

# 3. Fetch the actual data once status=completed
curl -s "https://api.chartbeat.com/query/v2/fetch/?query_id=<QID>" -H "X-CB-AK: <KEY>"
```

### Polling loop template

```bash
qid=$(curl -s "https://api.chartbeat.com/query/v2/submit/page/?host=example.com&start=2026-04-13&end=2026-04-20" \
  -H "X-CB-AK: <KEY>" | jq -r .query_id)
deadline=$(($(date +%s) + 30))
while :; do
  st=$(curl -s "https://api.chartbeat.com/query/v2/status/?query_id=$qid" -H "X-CB-AK: <KEY>" | jq -r .status)
  [ "$st" = "completed" ] && break
  [ "$st" = "failed" ] && { echo "query failed"; exit 1; }
  [ $(date +%s) -ge $deadline ] && { echo "timeout"; exit 1; }
  sleep 1
done
curl -s "https://api.chartbeat.com/query/v2/fetch/?query_id=$qid" -H "X-CB-AK: <KEY>"
```

### Legacy historical — still works with the header

```bash
# /historical/traffic/series/ accepts the header, returns series JSON directly.
curl -s "https://api.chartbeat.com/historical/traffic/series/?host=example.com&start=2026-04-19&end=2026-04-20&frequency=hour" \
     -H "X-CB-AK: <KEY>"
```

Prefer `/query/v2/...` for new queries; fall back to `/historical/...`
only if the modern path returns 404 for the specific metric.

## 3. Granularity for analysing dips

Live Publishing exposes traffic at **5-minute granularity** through
`/live/recent/v3` and friends. When the user says *"I see a dip
between 16h and 17h20"*, don't stop at hourly historical data — pull
minute-level live series, otherwise you miss the shape of the dip
(gradual vs brutal) and the rebound. 0-values at isolated timestamps
are almost always **API data gaps**, not real zero-traffic moments.

## 4. Host vs sub-domain — common pitfall

- `host=example.com` only sees the root-domain traffic. It does NOT
  aggregate sub-domains automatically.
- Regional editions use the full host: `host=de.example.com`. Check
  the site's client-side Chartbeat config to learn the exact host
  string (often `${locale}.${base}` built from a `Locale` helper).
- A 404 on `/historical/...` is almost always a wrong-auth or
  wrong-endpoint issue, NOT an API-key scope limit. Verify by:
  (a) switching to the header-based auth, (b) trying `/query/v2/...`
  variant, (c) checking the user's API key scope page shows `all`.

## 5. Params that work on both families

- `host=...` — tracked site (required)
- `limit=N` — cap rows
- `sections=news,sport` — filter
- `path=/article/xxx` — single URL filter
- `start=YYYY-MM-DD` / `end=YYYY-MM-DD` — date range
- `frequency=minute|hour|day` — time-series granularity

Official docs: https://docs.chartbeat.com/cbp/api/historical-api/getting-started-with-our-historical-api

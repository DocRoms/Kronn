# SpeedCurve — Usage Context

> Instructions for agents calling the **SpeedCurve** API via curl.

## 1. Auth — HTTP Basic with API key as user, empty password

```bash
curl -u "$SPEEDCURVE_API_KEY:" "https://api.speedcurve.com/v1/sites"
```

Note the trailing colon in `$KEY:` — the password half is intentionally
empty. Kronn already injects `Authorization: Basic <base64>` for you,
this is just for reference if the agent debugs raw auth.

## 2. Two data planes

- **Synthetic** (WebPageTest-style scheduled tests): `/v1/sites`,
  `/v1/tests`, `/v1/deploys`. Tracks Core Web Vitals + custom metrics
  on a schedule (every 30min, hourly, daily…) from chosen regions.
- **LUX** (Real User Monitoring — JS beacon on real user pages):
  `/v1/lux/...`. Aggregated CWV from real visitors, segmented by
  device/country/page-label/etc.

A single API key works for both.

## 3. Common workflows

- **Did this deploy break perf?** → POST `/v1/deploys` to mark a
  release timeline → next test run carries the deploy_id → compare
  `/v1/tests?since=<deploy_id>` against pre-deploy baseline.
- **CWV regression on prod?** → GET `/v1/lux/sites/<id>/metrics`
  with `metric=lcp&granularity=hour&start=<ts>&end=<ts>`.
- **Top slow URLs?** → GET `/v1/lux/sites/<id>/url_metrics`
  with `metric=lcp&order=desc&limit=20`.

## 4. Pagination

`limit` + `offset` (most endpoints, default limit 100, max 1000).
Some endpoints use `since` / `until` ISO-8601 timestamps for windowing.

## 5. Rate limit

300 req/min per key. Kronn's BatchApiCall default `concurrent_limit=5`
is well within bounds for typical fan-outs.

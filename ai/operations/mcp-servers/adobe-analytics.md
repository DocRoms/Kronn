# Adobe Analytics Reporting 2.0 — Usage Context

> Instructions for agents calling the **Adobe Analytics Reporting API 2.0**
> via curl. Kronn mints + caches the bearer token automatically.

## 1. How Kronn handles auth (so you don't have to)

Kronn exchanges `ADOBE_CLIENT_ID` + `ADOBE_CLIENT_SECRET` against
Adobe IMS (`https://ims-na1.adobelogin.com/ims/token/v3`) on every
discussion start (and on refresh when the 24h token is close to
expiring). The fresh bearer is injected into this context block as
`Authorization: Bearer <token>`. You just copy-paste it.

If the context says **"TOKEN UNAVAILABLE"**, stop and tell the user
— their `ADOBE_CLIENT_ID` / `SECRET` / `ORG_ID` are wrong or the
Adobe project isn't authorized for the Analytics API.

Three headers on EVERY Adobe call (Kronn surfaces them above; copy them
verbatim):
- `Authorization: Bearer <access_token>` (Kronn-managed)
- `x-api-key: <client_id>` (Adobe requires both)
- `x-proxy-global-company-id: <company_id>` (the analytics tenant)

Also:
- `Content-Type: application/json` on POST bodies

## 2. Base URL

```
https://analytics.adobe.io/api/<ADOBE_COMPANY_ID>/
```

Kronn interpolates `<ADOBE_COMPANY_ID>` automatically in the Base URL
field above. Endpoints are relative to that: `/reports`, `/dimensions`,
`/metrics`, etc.

## 3. The main endpoint — `POST /reports`

This is where 90 % of analyses go. It accepts a JSON body describing:
- `rsid`: the report suite (set `ADOBE_RSID` in the plugin config)
- `globalFilters`: at minimum a `dateRange` (ISO 8601 interval)
- `metrics`: what to count (`metrics/pageviews`, `metrics/visits`, etc.)
- `dimension`: ONE dimension to break down by (page, section, device…)
- `settings`: `{ "limit": 50 }` typical

### Pageviews by page over yesterday

```bash
curl -s -X POST "https://analytics.adobe.io/api/<COMPANY_ID>/reports" \
  -H "Authorization: Bearer <TOKEN>" \
  -H "x-api-key: <CLIENT_ID>" \
  -H "x-proxy-global-company-id: <COMPANY_ID>" \
  -H "Content-Type: application/json" \
  -d '{
    "rsid": "<RSID>",
    "globalFilters": [
      { "type": "dateRange", "dateRange": "2026-04-20T00:00:00/2026-04-20T23:59:59" }
    ],
    "metricContainer": {
      "metrics": [ { "id": "metrics/pageviews", "columnId": "0" } ]
    },
    "dimension": "variables/page",
    "settings": { "limit": 50, "page": 0 }
  }'
```

### Trended (time series) — minute granularity

Replace `dimension` with a time dimension:
- `variables/daterangeminute`  — per-minute
- `variables/daterangehour`    — per-hour
- `variables/daterangeday`     — per-day

```bash
# Pageviews per minute on a tight window (dip analysis)
-d '{
  "rsid": "<RSID>",
  "globalFilters": [
    { "type": "dateRange", "dateRange": "2026-04-20T14:00:00/2026-04-20T17:00:00" }
  ],
  "metricContainer": {
    "metrics": [ { "id": "metrics/pageviews", "columnId": "0" } ]
  },
  "dimension": "variables/daterangeminute",
  "settings": { "limit": 180 }
}'
```

### Segmenting

Apply a segment UUID (see `/segments` to list) as a `globalFilter`:
```json
{ "type": "segment", "segmentId": "s300000000_63abcdef1234" }
```

## 4. Other useful endpoints

- `GET /dimensions?rsid=<RSID>` — available dimensions for the RSID
- `GET /metrics?rsid=<RSID>` — available metrics
- `GET /segments?rsid=<RSID>` — saved segments
- `POST /reports/realtime` — realtime reports (last 15-30 min, different body shape)
- `GET /calculatedmetrics?rsids=<RSID>` — user-defined calculated metrics
- `GET /users/me` — smoke test: should return your Adobe user object

Full schema: [Adobe Analytics 2.0 API docs](https://developer.adobe.com/analytics-apis/docs/2.0/)

## 5. Common pitfalls

- **`401` on a freshly-started Kronn**: the token was already requested
  but the Adobe project isn't linked to the Analytics product profile.
  Fix it in Adobe Admin Console → Products → Analytics → Permissions.
- **`403 "Forbidden"`**: the user doesn't have access to the RSID you
  requested. Try a different RSID or escalate to Adobe admin.
- **Rate limits**: 120 requests/minute per client_id per tenant on
  Reporting 2.0. Don't fan out parallel queries for minute-by-minute —
  one trended query with 60-180 rows is cheaper.
- **Huge responses**: a `/reports` with `limit=200` on a wide
  dimension can return 1 MB+. Prefer `limit=25-50` + pagination via
  `settings.page` when digging deep.

## 6. What NOT to do

- Do NOT call `/reports/realtime` for historical data — it's capped at
  the last ~30 min.
- Do NOT fan out one call per day to build a week's trend — one call
  with `variables/daterangeday` + 7-day range returns all 7 points.
- Do NOT leak the bearer token back to the user. It's valid for 24h
  but still a live credential.

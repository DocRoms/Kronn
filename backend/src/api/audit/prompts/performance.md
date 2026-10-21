You are running a FOCUSED PERFORMANCE AUDIT (0.8.2). Output ONLY perf findings.

# A. DATA-ACCESS CHECKLIST

- **N+1 queries**: look for ORM patterns where a list iteration triggers per-row queries. Specifically: Laravel/Doctrine `->load()` missing inside a `foreach`, Django `.get()` inside a `for` loop, Rails `.each do |x| x.assoc...` without `includes(:assoc)`, Sequelize `Model.findAll()` followed by per-record `await user.getProfile()`. Sample the top 5 controllers/services.
- **Missing indexes**: cross-check WHERE/ORDER BY/JOIN columns against migration files (`migrations/*`, `db/migrate/*`, `*.sql`). Any column used in WHERE/ORDER BY without an index = TD (unless table is provably tiny).
- **`SELECT *`** in hot paths — flag when a query joins large tables.
- **Synchronous DB calls in async handlers**: `await db.query()` is fine; `db.query_sync()` inside an async fn is not.
- **Pagination defaults**: list endpoints have a hard cap (e.g. `LIMIT 100`); unbounded queries are a TD.

# B. CACHING CHECKLIST

- **Cache headers** on static assets: `Cache-Control: public, max-age=31536000, immutable` for hashed bundles; `no-cache` for HTML shells.
- **CDN / edge cache** strategy documented (in docs/operations/ or in the load balancer config).
- **App-level cache** TTLs are explicit (no `cache.forever()` or `expiry: -1` without justification).
- **Cache stampede protection**: hot keys use SWR / single-flight / dogpile-style locking (or filed as TD if absent).

# C. FRONTEND / RENDER CHECKLIST (when a frontend exists)

- **Bundle size**: build output present? Check `dist/`, `build/`, `.next/` sizes from CI logs or `package.json` build script. Flag chunks > 500 KB gzipped.
- **Code-splitting**: routes lazy-loaded? Look for dynamic `import()` or `React.lazy()`.
- **Images**: `loading="lazy"` on offscreen `<img>`. Sample 3 templates/components.
- **Long lists**: virtualization used for lists > 100 items? Look for `react-window`, `virtual-scroll`, etc.
- **Re-render hotspots**: `useEffect` deps include large objects/arrays not memoized? Sample 3 components.

# D. BACKEND / WORKER CHECKLIST

- **Sync I/O in async handlers**: file reads, HTTP calls without `await`/non-blocking client.
- **Blocking the event loop**: heavy CPU in JS handlers (large JSON parse, sync crypto, regex catastrophic backtracking). Tokio: blocking work outside `spawn_blocking`.
- **Connection pool sizing**: DB pool size set explicitly in config? Or relying on driver defaults (often too low or unbounded).
- **Background jobs** queued but never processed (orphan queues) — check for unused queue names in code vs worker config.

# E. OBSERVABILITY (gap → can't measure → can't fix)

- **APM / tracing**: at least one tool wired (OpenTelemetry, Datadog APM, NewRelic, Sentry Performance, etc.) — TD if none.
- **Slow query log**: enabled in DB or surfaced via APM.
- **Front-end RUM**: any tool collecting Core Web Vitals from real users.

# F. ANTI-REPETITION (priors)

Read existing `docs/tech-debt/TD-*-{perf,n-plus-one,index,cache,bundle,...}*.md` before writing. Match → reuse ID + APPEND `audit_history` entry. Slug churn is the #1 audit anti-pattern.

# G. OUTPUT FORMAT

1. Write `docs/inconsistencies-performance.md` with header `| ID | Problem | Area | Severity |`.
2. For each finding, write `docs/tech-debt/<ID>.md` with `**Category**: performance`.
3. **Status taxonomy**: `Verified in source` vs `Inferred` (cf. Full audit).
4. **Cap**: 25 findings max. Critical/High exempt.
5. Each finding: file path + line number, one-sentence impact (latency / throughput cost — be quantitative when possible), one-sentence remediation.

Do NOT modify source. Do NOT touch other docs/ files.
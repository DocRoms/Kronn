# Fastly — Usage Context

> Instructions for AI agents using **Fastly** MCP in this project.

**Server:** Hybrid plugin: official Fastly MCP (Go binary wrapping Fastly CLI)
for exploration, plus deterministic Fastly API calls for repeatable workflows.

## 0. Installation, authentication and readiness

Kronn's image bundles pinned Fastly CLI and MCP binaries. The container reads
the host CLI profile through the read-only `/host-home` mount. The API broker
resolves `fastly auth token` in memory when a request runs; the token is never
copied into Kronn's plugin settings, Docker environment, or generated MCP files.

Authenticate once on the host:

```bash
fastly auth login
fastly auth list
```

This local CLI session is the recommended source. As a recovery option, the
plugin configuration may store an encrypted `FASTLY_API_TOKEN` in Kronn. The
deterministic API broker uses it only when `fastly auth token` fails; it is never
rendered in an agent prompt or returned by the broker.

Use the plugin drawer's readiness action to check four distinct layers:
Fastly CLI, active authentication, a real authenticated API request, and the
official MCP executable. CLI, authentication and API are required for Fastly's
primary deterministic path. The exploratory MCP is optional outside the Docker
image: its absence stays visible but does not mark an otherwise working plugin
as broken.

## 1. Choose the cheapest interface

- Prefer an existing Quick API or deterministic `api_call` for repeatable reads
  such as services, domains, historical stats and usage.
- Use the MCP for exploration or operations that are not declared in the
  deterministic endpoint list.
- Use the raw CLI only as a manual fallback.

## 2. Performance rules (result size)

Service listings return 100K+ chars easily. Mitigations, in order of
effectiveness:

- `fastly_result_summary` first — get a digest before reading anything
- `fastly_result_query` with filters (see tool spec)
- `fastly_result_read` with small `limit` (5-10) for pagination

If a result overflows to disk, parse with `jq` or `python3`:
```bash
jq '.[0].text | fromjson | .data[] | {Name, ServiceID, ActiveVersion}' <file>
```

The MCP result format is `[{"type": "text", "text": "<JSON_STRING>"}]`
— the inner JSON has a `data` key containing the actual array.

## 3. Common operations

```
# List services
fastly_execute(command: "service", args: ["list"], flags: [{"name": "json"}])

# Stats — historical traffic for a service (by service-id, minute granularity)
fastly_execute(
  command: "stats",
  args: ["historical"],
  flags: [
    {"name": "service", "value": "<SERVICE_ID>"},
    {"name": "from",    "value": "2026-04-20 14:00:00"},
    {"name": "to",      "value": "2026-04-20 18:00:00"},
    {"name": "by",      "value": "minute"},
    {"name": "json"}
  ]
)

# Real-time stats (rolling window) — useful to correlate live traffic anomalies
fastly_execute(command: "stats", args: ["realtime"], flags: [{"name": "service", "value": "<SERVICE_ID>"}, {"name": "json"}])

# Purge by surrogate key
fastly_execute(command: "purge", args: ["--key", "<KEY>"], flags: [{"name": "service-id", "value": "<ID>"}])

# Domain listing
fastly_execute(command: "domain", args: ["list"], flags: [{"name": "service-id", "value": "<ID>"}, {"name": "version", "value": "active"}])
```

## 4. Traffic-correlation playbook

When the user reports a traffic anomaly in an external analytics tool
(Chartbeat, GA, etc.) and asks "is it the site or a Discover-style
referrer chute?", Fastly stats are the tie-breaker:

1. Find the service whose domain matches — `service list --json`, grep on
   domain name. Sub-domains often have their own service ID.
2. Pull `stats historical` at minute granularity over the suspect window.
3. Compare *hits* (edge requests served) vs *cache_miss* (backend hits):
   - Stable hits, normal cache ratio → the site was healthy; the dip
     is upstream (referrer algorithm, editorial, etc.).
   - Hit drop mirroring the analytics drop, cache ratio stable → traffic
     really fell at the edge — not a measurement artefact.
   - Hit drop + cache miss spike → origin slow / 5xx → site issue.

Surface both the Chartbeat-style number AND the Fastly hit number in
the final report so the user can judge for themselves.

## 5. Rules

- Always use `--json` flag when available to get structured output
- Never purge without explicit user confirmation
- Prefer `fastly_result_summary` to get an overview before reading full results
- If the CLI reports that no token is selected, stop and ask the user to run
  `fastly auth login` rather than
  guessing a service id

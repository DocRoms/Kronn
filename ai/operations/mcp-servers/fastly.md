# Fastly — Usage Context

> Instructions for AI agents using **Fastly** MCP in this project.

**Server:** Official Fastly MCP (Go binary wrapping Fastly CLI)

## 0. If `fastly CLI not found in PATH` — READ FIRST

The MCP shells out to the `fastly` CLI under the hood. Inside Kronn's Docker
container, three symptoms point to the same root cause:

- `fastly_execute` returns *"fastly CLI not found in PATH"*
- `which fastly` inside the container: not found
- But on the host, `fastly version` works fine

**Root cause**: on Linux/WSL, `npm i -g @fastly/cli` installs a JS wrapper
(`/usr/local/bin/fastly` → `../lib/node_modules/@fastly/cli/fastly.js`).
Kronn mounts `/usr/local/bin` but, until v0.5.0, did NOT mount
`/usr/local/lib`, so the relative symlink resolved to a non-existent
path inside the container. v0.5.0+ adds the `/host-bin/lib` mount which
fixes this transparently — if the problem persists, verify you're on
an up-to-date Kronn image (`./kronn version` / `make start`).

**Alternative fix that works on any Kronn version**: replace the JS
wrapper with the standalone Go binary from
[fastly/cli releases](https://github.com/fastly/cli/releases). The Go
binary is self-contained → no symlink gymnastics → works from any
mount layout.

Verify auth after install:
```bash
fastly profile list          # shows configured profiles
fastly auth list             # shows active tokens
fastly service list --json   # smoke test against the API
```

## 1. Performance rules (result size)

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

## 2. Common operations

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

## 3. Traffic-correlation playbook

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

## 4. Rules

- Always use `--json` flag when available to get structured output
- Never purge without explicit user confirmation
- Prefer `fastly_result_summary` to get an overview before reading full results
- If the CLI reports "no profile selected" → the token is missing;
  stop and ask the user to run `fastly profile create` rather than
  guessing a service id

# Post-release smoke checklist

Replayable verification pass for a freshly tagged/merged release, on a **copy**
of the real database — production is never pointed at. First executed for
0.8.11 (2026-07-14); versioned here so every release replays the same grid
and logs the same evidence (tag tested, commands, verdict, relevant logs) in
the release discussion.

**Hard rule:** every step that spawns or kills a process goes through
`scripts/smoke-bench.sh` (PID captured at spawn, bench marker, kill refusal
outside the bench). Never `pgrep <pattern> | head`: on this repo production
runs the same `target/debug/kronn` binary under `cargo watch`, and a pattern
match once killed production for ~2 minutes (0.8.11 smoke, P1).

## 1. Consistent DB copy

```bash
BENCH=$(mktemp -d /tmp/kronn-smoke-XXXX)
sqlite3 "file:$HOME/Library/Application Support/com.kronn.kronn/kronn.db?mode=ro" \
        ".backup '$BENCH/kronn.db'"          # backup API, read-only — never cp a live WAL
sqlite3 "$BENCH/kronn.db" "PRAGMA integrity_check;"   # expect: ok
```

- [ ] `integrity_check` = ok
- [ ] message count plausible vs production

## 2. Isolated boot on the copy

```bash
sed 's/^port = .*/port = 3141/' "$HOME/Library/Application Support/com.kronn.kronn/config.toml" \
    > "$BENCH/config.toml"                    # alternative port — never the prod one
scripts/smoke-bench.sh start "$BENCH"         # captures PID, marks the process
```

- [ ] latest migration recorded in `_migrations`, backfills applied
- [ ] `PRAGMA journal_mode` = wal AND no "skipping the read-only companion"
      warning in the boot log (that warning is the ONLY alternative path —
      its absence proves the read connection opened)
- [ ] zero `ERROR` in the boot log (benign PATH warnings from mcp_scanner
      are expected on a bench)

## 3. Heavy read endpoints

```bash
curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:3141/api/health
curl -s "http://127.0.0.1:3141/api/workflows"           # then GET /:id/runs
curl -s -o "$BENCH/export.zip" "http://127.0.0.1:3141/api/config/export"
python3 -c "import zipfile; print(zipfile.ZipFile('$BENCH/export.zip').testzip() or 'OK')"
```

- [ ] health 200 · run list 200 · export 200 and `testzip` OK

## 4. Crash / reconcile

```bash
scripts/smoke-bench.sh kill9 "$BENCH"         # refuses any PID without the bench marker
scripts/smoke-bench.sh start "$BENCH"
```

- [ ] encryption key reconciled from its existing source (**never** a silent
      regeneration — the 2026-06-30 incident class)
- [ ] orphan scan clean · health 200 after restart

## 5. Live multi-agent pass (production, observation only)

- [ ] MCP reload of one agent → same `session_pk` on rejoin, participant
      chips stable in the UI
- [ ] `pacing` present in wait/meta/join responses, regime coherent with the
      last human message

## 6. Log triage grid (24-48h burn-in window)

| Signal | Severity |
|---|---|
| `kronn::invariant` (any) | P1 — ticket with repro |
| `pacing anchors unavailable — cold fallback` | P1 — anchor SQL failing |
| `skipping the read-only companion` on a WAL filesystem | P1 |
| bridge transport retries exhausted | P2 — correlate with backend restarts |
| mcp_scanner missing binaries, in-volume backup warning | expected noise |

Every anomaly becomes a ticket with a reproduction, never an improvised fix
on the release branch.

## 7. Teardown

```bash
scripts/smoke-bench.sh stop "$BENCH"          # kills ONLY the bench PID
rm -rf "$BENCH"
```

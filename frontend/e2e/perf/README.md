# Performance regression tests

Specs in this folder profile the UI against a **seeded sandbox** of 250
projects + 500 discussions. They are *not* part of the default
`pnpm test:e2e` suite — they need a separate backend instance and would
otherwise pollute the dev DB.

## Why a sandbox

The user's real backend (`~/.config/kronn/kronn.db`) holds their actual
projects + discussions. Seeding 500 fake rows in there would corrupt
their workspace. Instead, the perf specs run against a backend launched
with `KRONN_DATA_DIR=/tmp/kronn-perf-sandbox`, leaving the real DB
untouched.

## How to run locally

```bash
SANDBOX=/tmp/kronn-perf-sandbox
PORT=3142
VITE_PORT=61373        # not 5173: that is the developer's own Vite

# 1. Build the backend if you haven't. No full build is needed later.
make build-backend  # or `cd backend && cargo build`

# 2. Create and migrate the sandbox, through the launcher that owns it.
#    It writes config.toml itself, refuses 3140/5173, refuses a port already
#    served, refuses a directory it did not create, and starts the backend
#    under `env -i` with explicit roots — KRONN_HOST_HOME included.
#
#    That last one is not cosmetic: at boot the backend syncs ~/.claude.json,
#    ~/.gemini/settings.json and ~/.codex/config.toml, dropping every
#    Kronn-managed entry before re-inserting from the database it was given.
#    An empty sandbox database therefore STRIPS your own MCP configuration.
#    That happened on 2026-09-13.
PID=$(scripts/e2e-sandbox-backend.sh "$SANDBOX" "$PORT")

# 3. Stop it BEFORE seeding, and WAIT for it to be gone. `seed.py` writes rows
#    straight into kronn.db and creates no schema of its own, so it needs the
#    migrated database step 2 just produced — and it must not race a writer
#    that still holds the file. `kill` only asks; it does not wait.
kill "$PID"
for _ in $(seq 1 100); do kill -0 "$PID" 2>/dev/null || break; sleep 0.1; done
kill -0 "$PID" 2>/dev/null && { echo "backend $PID did not exit; stop here"; exit 1; }

# 4. Seed. This deletes the sandbox DB's own rows (messages, discussions,
#    projects) and writes 250 projects + 500 discussions + ~7500 messages.
#    It removes no files and touches nothing outside that database.
python3 frontend/e2e/perf/seed.py

# 4-bis. (Optional) Add the 20-msg discussion the introspection regression
#    spec needs. Idempotent — re-run without conflicts.
python3 frontend/e2e/perf/seed_introspection.py

# 5. Restart on the seeded database. The launcher owns only directories it
#    creates and refuses this one now, so its isolation is repeated here
#    VERBATIM — same `env -i`, same explicit roots. Weakening any of it is how
#    an ambient credential, proxy or keychain leaks into a throwaway instance.
( cd "$SANDBOX"
  exec env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin \
      KRONN_DATA_DIR="$SANDBOX" KRONN_HOST=127.0.0.1 \
      KRONN_HOST_HOME="$SANDBOX/host-home" \
      KRONN_BACKEND_URL="http://127.0.0.1:$PORT" KRONN_USE_KEYCHAIN=0 \
      KRONN_BACKUP_DIR="$SANDBOX/backups" KRONN_BACKUP_INTERVAL_HOURS=0 \
      KRONN_DOCS_SIDECAR="$SANDBOX/docs-sidecar-disabled" \
      XDG_CONFIG_HOME="$SANDBOX/xdg-config" XDG_CACHE_HOME="$SANDBOX/cache" \
      TMPDIR="$SANDBOX/tmp" "$PWD/target/debug/kronn"
) < /dev/null >"$SANDBOX/backend.log" 2>&1 &
PID=$!

# 6. Prove it is healthy AND that it is ours. A 200 says something answered,
#    not that this process did: a stale sandbox still holding the port answers
#    everything, including after its data directory is gone.
until curl -fsS --max-time 5 "http://127.0.0.1:$PORT/api/health" 2>/dev/null \
      | grep -q '"ok":true'; do
    kill -0 "$PID" 2>/dev/null || { echo "backend exited: $(tail -1 "$SANDBOX/backend.log")"; exit 1; }
    sleep 1
done
[ "$(lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t | head -1)" = "$PID" ] \
    || { echo "port $PORT is served by someone else"; kill "$PID"; exit 1; }

# 7. Vite, on its own port, pointed at the sandbox.
( cd frontend && env KRONN_BACKEND_URL="http://127.0.0.1:$PORT" \
    pnpm exec vite --port "$VITE_PORT" ) &

# 8. Run the perf specs against that Vite.
cd frontend && env PLAYWRIGHT_BASE_URL="http://localhost:$VITE_PORT" \
    pnpm exec playwright test --config=playwright.perf.config.ts
```

When you are done: `kill "$PID"`, stop Vite, and delete `$SANDBOX` — it holds a
database seeded from your machine and is not meant to outlive the run.

### Running just the introspection regression

The introspection spec is API-only — it doesn't need Vite. Skip steps 4
above and point Playwright directly at the sandbox backend:

```bash
cd frontend && \
  KRONN_INTROSPECTION_BASE=http://localhost:3142 \
  pnpm exec playwright test --config=playwright.perf.config.ts \
  introspection.perf.spec.ts
```

## Thresholds

The specs assert the following budgets against the seeded DB. They
allow ~30 % headroom over the post-fix Chromium-on-WSL numbers I
measured on 2026-05-09 — slower CI machines have margin without making
the tests false-flag.

| Action | Budget | Measured (post-fix) |
|---|---|---|
| Dashboard search keystroke | < 250 ms | 148 ms |
| Sidebar search keystroke | < 800 ms | 472 ms |
| Discussions cold render (lazy chunk + sidebar) | < 3000 ms | 1852 ms |
| Sidebar scroll p95 | < 50 ms | 19 ms |
| Sidebar loose-disc cap | "+ N more" appears for groups > 10 disc | 5 buttons |
| Introspection bridge — meta + msg + ranged cache | All endpoints respond and call counter bumps ≥ 6× | n/a (correctness) |

## Why not in CI

The seeded backend takes ~60 s warm-up on first migration + Vite
cold-start, which would slow the green CI loop. Run these manually
before a release or after a perf-sensitive refactor.

## Large-message render guard (2026-06-23)

A killed agent (Codex `exec`, silent-until-end) once persisted a 2.4 MB
stderr/reasoning dump as its reply; opening that discussion sent it through
ReactMarkdown + syntax highlight and **crashed the browser tab**. Two layers
now prevent it: a backend byte-cap at persistence (`cap_agent_response`) and a
frontend plain-text fallback past ~200 KB (`MarkdownContent`).

Run the real-browser regression check:

```bash
# after booting the sandbox backend (see "Setup" above)
python3 e2e/perf/seed_large_message.py          # adds ONE 3 MB-message disc
pnpm exec playwright test --config=playwright.perf.config.ts large-message-render
```

| Action | Budget | Notes |
|---|---|---|
| Open a ~3 MB-message discussion → guard banner visible | < 8000 ms | pre-fix: timed out / tab crashed |
| Inline rendered chars (DOM) | < 200 000 | guard truncates; full payload never mounted |

The unit test `MessageBubble.largeMessage.test.tsx` pins the guard logic in CI
(jsdom can't catch a real OOM — hence this real-Chromium spec).

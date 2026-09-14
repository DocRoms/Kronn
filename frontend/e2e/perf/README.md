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
# The path is FIXED, not a preference: seed.py and seed_introspection.py both
# hard-code /tmp/kronn-perf-sandbox. Running this "somewhere private" while the
# seeds still write there would be a qualification that proves nothing.
SANDBOX=/tmp/kronn-perf-sandbox
PORT=3142
VITE_PORT=61373          # not 5173: that is the developer's own Vite

# Capture the binary BEFORE any `cd`. Step 5 runs inside the sandbox, where a
# relative path — or $PWD — resolves to the sandbox and starts nothing.
KRONN_BIN="$PWD/target/debug/kronn"

# Stop only what this runbook started, on any exit.
BACKEND_PID=""; VITE_PID=""
stop_owned() {
    for pid in "$VITE_PID" "$BACKEND_PID"; do
        [ -n "$pid" ] || continue
        kill "$pid" 2>/dev/null || true
        for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.1; done
        kill -KILL "$pid" 2>/dev/null || true
    done
}
trap stop_owned EXIT

# 1. Build the backend if you haven't. Nothing later needs a build.
make build-backend  # or `cd backend && cargo build`

# 2. Create and migrate the sandbox, through the launcher that owns it. It
#    writes config.toml itself, refuses 3140/5173, refuses a port already
#    served, refuses a directory it did not create, and starts the backend
#    under `env -i` with explicit roots — KRONN_HOST_HOME included.
#
#    That last one is not cosmetic: at boot the backend syncs ~/.claude.json,
#    ~/.gemini/settings.json and ~/.codex/config.toml, dropping every
#    Kronn-managed entry before re-inserting from the database it was given.
#    An empty sandbox database therefore STRIPS your own MCP configuration.
#    That happened on 2026-09-13.
#    The launcher owns only a directory it creates, and refuses one it finds.
#    Do NOT delete this path to make room: it is fixed, it is not ours, and a
#    published `rm -rf` on a fixed location is how somebody else's work
#    disappears. If it exists, stop and find out what is holding it.
[ -e "$SANDBOX" ] && { echo "$SANDBOX already exists — find out what owns it, then remove it yourself"; exit 1; }
BACKEND_PID=$(scripts/e2e-sandbox-backend.sh "$SANDBOX" "$PORT")

# 3. Stop it BEFORE seeding, and WAIT. `seed.py` writes rows straight into
#    kronn.db and creates no schema of its own, so it needs the migrated
#    database step 2 just produced — and it must not race a writer still
#    holding the file. `kill` only asks; it does not wait.
kill "$BACKEND_PID"
for _ in $(seq 1 100); do kill -0 "$BACKEND_PID" 2>/dev/null || break; sleep 0.1; done
kill -0 "$BACKEND_PID" 2>/dev/null && { echo "backend did not exit; stop here"; exit 1; }
BACKEND_PID=""

# 4. Seed. This deletes rows from three tables of the sandbox database and
#    writes 250 projects + 500 discussions + ~7500 messages. It removes no
#    files and touches nothing outside that database.
python3 frontend/e2e/perf/seed.py

# 4-bis. (Optional) Add the 20-msg discussion the introspection regression
#    spec needs. Idempotent — re-run without conflicts.
python3 frontend/e2e/perf/seed_introspection.py

# 5. Restart on the seeded database. The launcher owns only directories it
#    creates and refuses this one now, so its isolation is repeated here
#    VERBATIM — same `env -i`, same explicit roots. Weakening any of it is how
#    an ambient credential, proxy or keychain reaches a throwaway instance.
( cd "$SANDBOX"
  exec env -i PATH=/usr/bin:/bin:/usr/sbin:/sbin \
      KRONN_DATA_DIR="$SANDBOX" KRONN_HOST=127.0.0.1 \
      KRONN_HOST_HOME="$SANDBOX/host-home" \
      KRONN_BACKEND_URL="http://127.0.0.1:$PORT" KRONN_USE_KEYCHAIN=0 \
      KRONN_BACKUP_DIR="$SANDBOX/backups" KRONN_BACKUP_INTERVAL_HOURS=0 \
      KRONN_DOCS_SIDECAR="$SANDBOX/docs-sidecar-disabled" \
      XDG_CONFIG_HOME="$SANDBOX/xdg-config" XDG_CACHE_HOME="$SANDBOX/cache" \
      TMPDIR="$SANDBOX/tmp" "$KRONN_BIN"
) < /dev/null >"$SANDBOX/backend.log" 2>&1 &
BACKEND_PID=$!

# 6. Prove it is healthy AND that it is ours, within a bounded number of tries.
#    A 200 says something answered, not that this process did: a stale sandbox
#    still holding the port answers everything. `--noproxy` because an ambient
#    proxy would otherwise answer for localhost.
healthy=""
for _ in $(seq 1 90); do
    kill -0 "$BACKEND_PID" 2>/dev/null \
        || { echo "backend exited: $(tail -1 "$SANDBOX/backend.log")"; exit 1; }
    if curl -q --noproxy '*' -fsS --max-time 5 \
         "http://127.0.0.1:$PORT/api/health" 2>/dev/null | grep -q '"ok":true'; then
        healthy=yes; break
    fi
    sleep 1
done
[ -n "$healthy" ] || { echo "no healthy answer on $PORT"; exit 1; }
[ "$(lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t | head -1)" = "$BACKEND_PID" ] \
    || { echo "port $PORT is served by someone else"; exit 1; }

# 7. Vite, on its own port, pointed at the sandbox. `--strictPort` so it fails
#    instead of silently sliding onto another port — including 5173.
( cd frontend && exec env KRONN_BACKEND_URL="http://127.0.0.1:$PORT" \
    pnpm exec vite --port "$VITE_PORT" --strictPort ) &
VITE_PID=$!

# 8. Run the perf specs against that Vite.
( cd frontend && env PLAYWRIGHT_BASE_URL="http://localhost:$VITE_PORT" \
    pnpm exec playwright test --config=playwright.perf.config.ts )
```

The `trap` stops the backend and Vite on the way out, by pid, and nothing else.
Delete `$SANDBOX` when you are done — it holds a database seeded from your
machine and is not meant to outlive the run.

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

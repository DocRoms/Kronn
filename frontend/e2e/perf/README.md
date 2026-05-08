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
# 1. Build the backend if you haven't.
make build-backend  # or `cd backend && cargo build`

# 2. Seed the sandbox DB. The script wipes any existing /tmp/kronn-perf-
#    sandbox/* and writes 250 projects + 500 discussions + ~7500 messages.
python3 frontend/e2e/perf/seed.py

# 2-bis. (Optional) Add the 20-msg discussion the introspection regression
#    spec needs. Idempotent — re-run without conflicts.
python3 frontend/e2e/perf/seed_introspection.py

# 3. Boot the sandbox backend on :3142 (won't collide with the user's
#    real backend on :3140).
env KRONN_DATA_DIR=/tmp/kronn-perf-sandbox \
    backend/target/debug/kronn &

# 4. Boot Vite pointed at the sandbox.
env KRONN_BACKEND_URL=http://localhost:3142 \
    pnpm --filter kronn-frontend dev &

# 5. Run the perf specs.
cd frontend && pnpm exec playwright test --config=playwright.perf.config.ts
```

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

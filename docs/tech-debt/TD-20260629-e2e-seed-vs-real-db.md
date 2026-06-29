# TD-20260629-e2e-seed-vs-real-db

- **ID**: TD-20260629-e2e-seed-vs-real-db
- **Area**: CI / Frontend (E2E)
- **Problem (fact)**: Several Playwright E2E specs are **calibrated on a freshly-seeded DB** (fixed baselines / preconditions / time budgets), so they produce **false reds when run against a real, rich, stateful DB**. Observed on the macOS-PR WSL validation (8/72 failed, all reproduced identically on `main` → not code-related):
  - **a11y contrast baselines** (`a11y-baseline.json`): the seed assumes a small element count; a real DB (20 projects · 42 skills · 7 agents) renders more elements → violation counts exceed the pinned baseline (e.g. Projects `64 > 51`, Settings `112 > 36`) even when the PR changes **zero** color styles.
  - **`audit-banner-lifecycle`**: precondition "no audit in progress" is violated when a project happens to be mid-audit in the DB.
  - **introspection / "real agent run" specs**: the **10 s budget** is too tight for a **cold first MCP call** (~13 s measured; the feature works, per backend logs).
- **Why we can't fix now (constraint)**: Pure test-harness robustness, not a product bug — out of scope of the PR that surfaced it. **CI is unaffected** (the `test-e2e` job seeds a fresh `KRONN_DATA_DIR` + bypasses the wizard), so these only bite **manual runs against a real instance**. Fixing well means reworking baselines/preconditions, not a one-liner.
- **Impact**: test fragility (false reds on local / real-DB runs; erodes trust in the E2E signal)
- **Where (pointers)**:
  - `frontend/e2e/` — the a11y specs + `a11y-baseline.json`, `audit-banner-lifecycle*.spec.*`, the introspection / "real agent run" specs.
  - The E2E seed/setup (`frontend/e2e/perf/seed.py` + the `test-e2e` job's setup steps in `.github/workflows/ci-test.yml`).
- **Suggested direction (non-binding)**:
  - **a11y**: make baselines **state-relative** (assert "no NEW violations vs a baseline captured on the same DB state", or scope axe to a stable region) instead of absolute element counts.
  - **audit-banner**: force a known state in the spec setup (cancel/await any in-progress audit, or stub `/api/projects/:id/audit-status`) rather than assuming the DB is idle.
  - **introspection budget**: bump the cold-first-call timeout to ~20 s (or warm the MCP before asserting).
- **Next step**: create ticket.

## Notes

- Surfaced 2026-06-29 during the WSL pre-merge validation of `fix/macos-errors`
  (PR #108). All 8 E2E failures were disculpated by reproducing them on `main`
  with the same DB — the PR diff touches none of Projects/Workflows/introspection/
  audit-banner/Settings-color. Tracked here so the "résiduels" become actionable.

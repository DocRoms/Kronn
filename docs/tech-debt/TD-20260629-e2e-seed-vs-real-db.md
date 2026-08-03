# TD-20260629-e2e-seed-vs-real-db

- **ID**: TD-20260629-e2e-seed-vs-real-db
- **Area**: CI / Frontend (E2E)
- **Status**: PARTIAL — isolated local runs are now first-class; state-relative
  baselines and the remaining contrast remediation remain open.
- **Problem (fact)**: Several Playwright E2E specs are **calibrated on a freshly-seeded DB** (fixed baselines / preconditions / time budgets), so they produce **false reds when run against a real, rich, stateful DB**. Observed on the macOS-PR WSL validation (8/72 failed, all reproduced identically on `main` → not code-related):
  - **a11y contrast baselines** (`a11y-baseline.json`): the seed assumes a small element count; a real DB (20 projects · 42 skills · 7 agents) renders more elements → violation counts exceed the pinned baseline (e.g. Projects `64 > 51`, Settings `112 > 36`) even when the PR changes **zero** color styles.
  - **`audit-banner-lifecycle`**: precondition "no audit in progress" is violated when a project happens to be mid-audit in the DB.
  - **introspection / "real agent run" specs**: the **10 s budget** is too tight for a **cold first MCP call** (~13 s measured; the feature works, per backend logs).
- **Why it remains partial**: The harness assumptions can be made deterministic,
  but an absolute axe node count still varies with the rendered dataset. The
  remaining color-contrast findings are product accessibility debt and must be
  fixed in the theme/components rather than hidden by a wider test allowance.
- **Impact**: test fragility (false reds on local / real-DB runs; erodes trust in the E2E signal)
- **Where (pointers)**:
  - `frontend/e2e/` — the a11y specs + `a11y-baseline.json`, `audit-banner-lifecycle*.spec.*`, the introspection / "real agent run" specs.
  - The E2E seed/setup (`frontend/e2e/perf/seed.py` + the `test-e2e` job's setup steps in `.github/workflows/ci-test.yml`).
- **Suggested direction (non-binding)**:
  - **a11y**: make baselines **state-relative** (assert "no NEW violations vs a baseline captured on the same DB state", or scope axe to a stable region) instead of absolute element counts.
  - **audit-banner**: force a known state in the spec setup (cancel/await any in-progress audit, or stub `/api/projects/:id/audit-status`) rather than assuming the DB is idle.
  - **introspection budget**: bump the cold-first-call timeout to ~20 s (or warm the MCP before asserting).
- **Next step**: replace the absolute axe counts with a state-relative fixture
  or stable scan region, then fix the remaining contrast nodes to zero.

## Shipped mitigation (0.9.0)

`playwright.config.ts` now honours `VITE_DEV_PORT`, while both Vite and the
direct-backend E2E calls honour `KRONN_BACKEND_URL`. A developer can therefore
run Playwright against a fresh backend/data directory on parallel ports without
stopping or mutating the real Kronn instance. This removes the main operational
friction that caused accidental rich-DB runs; it does not make the absolute
accessibility baselines or audit preconditions state-relative.

## Shipped mitigation (0.9.3)

- Automation page objects finish navigation through the active-runs popover
  when a real workflow is running, so a busy operator instance no longer turns
  the suite into retries.
- Audit lifecycle/card scenarios own their route-mocked state, and billed
  real-agent canaries require the explicit `KRONN_REAL_AGENT_E2E=1` opt-in.
- Guided-tour replay waits for the mounted dashboard and targets the localized
  accessible help-button name; backend recovery uses deterministic local 503 →
  200 responses.
- The reviewed axe snapshot is now Projects 5, Discussions 0, Plugins 1,
  Automation 2 and Settings 2. This is a regression ceiling, not an assertion
  that the 10 remaining serious contrast nodes are acceptable long-term.

With those mitigations, the 0.9.3 standard local run completed with 77 passing,
11 explicit/conditional skips, zero failures and zero retries/flaky results on
the rich release instance. The state-relative baseline design and contrast
remediation remain open.

## Notes

- Surfaced 2026-06-29 during the WSL pre-merge validation of `fix/macos-errors`
  (PR #108). All 8 E2E failures were disculpated by reproducing them on `main`
  with the same DB — the PR diff touches none of Projects/Workflows/introspection/
  audit-banner/Settings-color. Tracked here so the "résiduels" become actionable.

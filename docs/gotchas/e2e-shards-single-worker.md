# E2E shards: three runners, one Playwright worker each

**Symptom.** The `test-e2e` job spent 12 min 48 s of its 17 min on the 202
Playwright tests, one worker, one runner (run 38747093853, 2026-10-15).

**Why not more workers.** The Rust backend is one process and serialises
config writes, agent detection and MCP scans: three Playwright workers hitting
`/api/setup/status` together hang it, so `workers: 1` is deliberate.
[src: file: frontend/playwright.config.ts:49-59]

**Rule.** Parallelism comes from the job matrix, not from Playwright workers.
`test-e2e` runs `shard: [1, 2, 3]`; each shard is its own runner that builds
and starts its own backend, then runs `playwright test --shard=N/3` with the
unchanged config. Failure artifacts carry the shard number because
`upload-artifact@v4` artifacts are immutable and a duplicate name fails the
upload. A push to `main` runs one shard only: it builds and saves the cache,
it does not test.
[src: file: .github/workflows/ci-test.yml:672-694]
[src: url: https://github.com/actions/upload-artifact/blob/main/README.md]

**Balance.** Playwright deals whole spec files to shards (file-level
granularity), so a spec with many tests, such as `a11y-dark-themes.spec.ts`
with 27, tips its shard. At the time of writing the split is 68 / 68 / 66.
Check it without a backend:
[src: url: https://playwright.dev/docs/test-sharding]

```bash
cd frontend && CI=true pnpm exec playwright test --list --shard=1/3
```

**Check.** The three `Playwright Run Summary` counts in the shard logs must add
up to the count of `playwright test --list` on the same commit. The
`ci-quality-gates` job lists `test-e2e` once: a matrix job's `needs` result is
the aggregate, so one red shard keeps the gate red.

Related: [actions-cache-scope](actions-cache-scope.md),
[testing-quality](../testing-quality.md).

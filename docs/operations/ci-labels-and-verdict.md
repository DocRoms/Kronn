# CI labels and verdict

A pull request runs its heavy CI only once a maintainer adds a label:
`ci-test` starts **CI Tests** (`ci-test.yml`), `ci-build` starts **CI Build**
(`ci-build.yml`). Each workflow ends in one aggregate, `ci-quality-gates` and
`ci-build-gates`, and the `main` ruleset requires those two aggregates plus the
real jobs (`test-backend`, `test-frontend`, `security-scan`…), never the
`require-ci-*-label` guards, which pass on the label alone.

## Why a label event shows a second verdict

A label change, or an edit, tests nothing new, so CI Tests and CI Build do not
run for it. **CI Verdict** (`ci-verdict.yml`) runs instead and republishes the
two aggregates from the run that tested the same pull request, head and base
(`scripts/ci/previous_gate_verdict.sh`). If that run predates the label, it is
re-run once and its new verdict is repeated.

- Runs of CI Verdict for one pull-request state share a concurrency group:
  a burst of label events keeps only the newest run.
- GitHub keeps every check run it ever received for a SHA. An older red
  aggregate can stay listed next to a newer green one; the required check reads
  the newest.

## Cancelled jobs

A cancelled job is not a failure of the code, but nothing passed either:

- If a gate job was cancelled before deciding, CI Verdict looks at earlier runs
  for the same state, and asks for a re-run of the cancelled one if none decided.
- If a gate failed because one of its jobs was cancelled (a runner shutdown, a
  cancelled macOS portability job), the error names those jobs. Use
  "Re-run failed jobs" on that run; CI Verdict then repeats the new attempt.

`tests/bats/previous_gate_verdict.bats` runs the verdict script against a stub
`gh` for these cases.

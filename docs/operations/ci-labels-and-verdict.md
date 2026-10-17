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
- GitHub keeps every check run it ever received for a SHA and cannot delete
  one. So the run that finishes **supersedes** the earlier verdicts
  (`scripts/ci/supersede_verdicts.sh`, last step of each gate job).

## Superseded verdicts

When a CI Verdict run has reported `ci-quality-gates` or `ci-build-gates`, it
updates the earlier check runs of the **same head SHA and the same gate name**
to conclusion `neutral`, titled `Superseded by run <id>`, with a summary that
links the latest run. The history stays visible; the latest verdict is the only
success or failure of that gate on the head.

A check run is superseded only if all of these hold:

- it has the gate's name and is completed and not yet `neutral` (so a second
  pass is a no-op);
- its `details_url` points at a **CI Verdict** run of this head SHA with a lower
  run id. CI Tests and CI Build publish checks with the same names on the same
  SHA: those are never touched;
- the current run is not cancelled (the step runs on success or failure only):
  a run replaced by a newer label event supersedes nothing, its successor does.

Superseding is cosmetic and never turns a verdict red: a read-only token (a
fork) or an API error prints a warning and exits 0, and the old check stays
listed. The job needs `checks: write`.

`backend/scripts/test_ci_supersede_verdicts.py` covers which runs are selected
and the fields sent, against a stub `gh`:

```bash
python3 -m unittest backend/scripts/test_ci_supersede_verdicts.py
```

## Which checks must be required

Branch protection must require the aggregates `ci-quality-gates` and
`ci-build-gates`, not the `require-ci-label` / `require-ci-build-label` guards:
a guard succeeds on the label alone, whatever the tests did. Read the live
ruleset (needs repository admin read access):

```bash
gh api repos/DocRoms/Kronn/rulesets/13870406 \
  --jq '.rules[] | select(.type == "required_status_checks")
        | .parameters.required_status_checks[] | .context'
```

The list must contain both aggregates and neither guard. If an aggregate is
missing, append it as described in `CONTRIBUTING.md`. A `neutral` superseded
check never blocks a merge, so the ruleset needs no change for it.

## Cancelled jobs

A cancelled job is not a failure of the code, but nothing passed either:

- If a gate job was cancelled before deciding, CI Verdict looks at earlier runs
  for the same state, and asks for a re-run of the cancelled one if none decided.
- If a gate failed because one of its jobs was cancelled (a runner shutdown, a
  cancelled macOS portability job), the error names those jobs. Use
  "Re-run failed jobs" on that run; CI Verdict then repeats the new attempt.

`tests/bats/previous_gate_verdict.bats` runs the verdict script against a stub
`gh` for these cases.

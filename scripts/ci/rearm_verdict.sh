#!/usr/bin/env bash
# Re-runs the CI Verdict gate job that gave up waiting for a CI Tests / CI Build
# run, once that run has completed.
#
# previous_gate_verdict.sh keeps a still-running upstream run pending; past its
# job timeout GitHub ends the job as cancelled, which blocks the merge but is
# not a result. Nothing else would clear it, so ci-verdict-rearm.yml calls this
# when CI Tests or CI Build completes for a pull-request head.
#
# Only the newest CI Verdict run of that head is looked at, and only its gate
# job for the completed workflow. That job is re-run when
#   - it was cut short (a cancelled or timed-out conclusion, or a cancelled
#     step), or
#   - it failed: a verdict that ran on the label event before the upstream run
#     existed, or against an earlier attempt of it, is red for a result that has
#     since changed (KT-1523). A job that repeated the right failure just
#     repeats it again: the re-run is cheap and idempotent.
# A job that passed already told the truth and is left alone, and so is a gate
# that failed on its own label guard ("Require the <label> label": the label is
# absent, a newer label event starts a newer verdict run, a re-run changes
# nothing). A failed gate is re-armed at most REARM_MAX_ATTEMPTS attempts, so a
# verdict that keeps re-running its upstream run cannot loop. The re-run job
# reads the upstream result itself: this script never decides a verdict, so an
# unknown result can never turn green here.
#
# The run must be completed before one of its jobs can be re-run, and its
# sibling gate may still be polling: the script waits for that (REARM_WAIT_SECONDS).
# Every outcome exits 0: a red check of this workflow would land on the default
# branch, not on the pull request.
#
# API only. It reads no pull-request file and runs no pull-request code; the
# head SHA is data, validated before use.
#
# Environment: GH_TOKEN, GITHUB_REPOSITORY, UPSTREAM_NAME ("CI Tests" or
# "CI Build"), HEAD_SHA; optional REARM_WAIT_SECONDS (default 900),
# REARM_POLL_SECONDS (default 15), REARM_MAX_ATTEMPTS (default 3),
# VERDICT_WORKFLOW (default ci-verdict.yml).
set -uo pipefail

: "${GITHUB_REPOSITORY:?}" "${UPSTREAM_NAME:?}" "${HEAD_SHA:?}"
repo="repos/$GITHUB_REPOSITORY"
workflow="${VERDICT_WORKFLOW:-ci-verdict.yml}"

case "$UPSTREAM_NAME" in
  "CI Tests") gate=ci-quality-gates ;;
  "CI Build") gate=ci-build-gates ;;
  *)
    echo "$UPSTREAM_NAME has no gate in $workflow: nothing to re-arm."
    exit 0
    ;;
esac
if ! [[ "$HEAD_SHA" =~ ^[0-9a-f]{40}$ ]]; then
  echo "::warning::Not a commit SHA, nothing re-armed."
  exit 0
fi

deadline=$(( $(date +%s) + ${REARM_WAIT_SECONDS:-900} ))
poll="${REARM_POLL_SECONDS:-15}"
max_attempts="${REARM_MAX_ATTEMPTS:-3}"
if ! [[ "$max_attempts" =~ ^[0-9]+$ ]]; then
  max_attempts=3
fi

if ! run="$(gh api "$repo/actions/workflows/$workflow/runs?event=pull_request&head_sha=$HEAD_SHA&per_page=100" \
  --jq "[.workflow_runs[] | select(.head_sha == \"$HEAD_SHA\")] | sort_by(.id) | last | .id // empty")"; then
  echo "::warning::Could not list the $workflow runs of $HEAD_SHA; $gate is left as it is."
  exit 0
fi
if [ -z "$run" ]; then
  echo "No $workflow run for head $HEAD_SHA: nothing to re-arm."
  exit 0
fi

while :; do
  status="$(gh api "$repo/actions/runs/$run" --jq .status 2>/dev/null)" || status=""
  if [ "$status" = "completed" ]; then
    # "<job id> <needs a re-run>"; empty when the run has no such job.
    job="$(gh api "$repo/actions/runs/$run/jobs?per_page=100" \
      --jq "[.jobs[] | select(.name == \"$gate\")] | last
        | if . == null then \"\"
          else (.status == \"completed\") as \$done
            | ([.steps[]? | select(.conclusion == \"cancelled\")] | length > 0) as \$cut
            | ([.steps[]? | select((.name | test(\"^Require the .+ label\$\")) and .conclusion == \"failure\")] | length > 0) as \$unlabelled
            | (\$done and (.conclusion == \"cancelled\" or .conclusion == \"timed_out\" or (.conclusion == \"failure\" and \$cut))
                or (\$done and .conclusion == \"failure\" and (\$unlabelled | not) and (.run_attempt // 1) < $max_attempts)) as \$rearm
            | \"\(.id) \(\$rearm)\"
          end" 2>/dev/null)" || job=""
    if [ "${job##* }" != "true" ]; then
      echo "Run $run did not give up on $gate: nothing to re-arm."
      exit 0
    fi
    if gh api -X POST "$repo/actions/jobs/${job%% *}/rerun" >/dev/null 2>&1; then
      echo "Re-ran $gate of CI Verdict run $run: $UPSTREAM_NAME completed for head $HEAD_SHA."
      exit 0
    fi
    echo "Could not re-run $gate of run $run yet."
  fi
  if [ "$(date +%s)" -ge "$deadline" ]; then
    echo "::warning::Gave up re-arming $gate of run $run (status: ${status:-unknown}). Re-run CI Verdict by hand: https://github.com/$GITHUB_REPOSITORY/actions/runs/$run"
    exit 0
  fi
  sleep "$poll"
done

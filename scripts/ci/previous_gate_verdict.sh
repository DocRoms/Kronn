#!/usr/bin/env bash
# Exits with the verdict a CI Tests or CI Build run gave for this exact
# pull-request state. CI Verdict calls it on label and edit events, which those
# workflows never run for, so the aggregate repeats the real verdict instead of
# reporting a skip (a skipped required check counts as green).
#
# Evidence is the gate's verdict step, whose name records the identity it
# tested: "Gate verdict for pull_request #<n> head <sha> base <sha>". A
# dispatch, another pull request or another base never carries that step, so
# it is never accepted. Earlier means a lower run id.
#
# A run that saw no label (its "Require the <label> label" step failed) is
# re-run once: the caller checked the label is present now, and the re-run
# reads labels live. Its new attempt's verdict is then repeated.
#
# Usage: previous_gate_verdict.sh <workflow file> <gate job name> <label>
# Environment: GH_TOKEN, GITHUB_REPOSITORY, GITHUB_RUN_ID, PR_NUMBER, HEAD_SHA,
# BASE_SHA.
set -euo pipefail

workflow="$1"
job="$2"
label="$3"
: "${GITHUB_REPOSITORY:?}" "${GITHUB_RUN_ID:?}" "${PR_NUMBER:?}" "${HEAD_SHA:?}" "${BASE_SHA:?}"
wait_seconds="${PREVIOUS_VERDICT_WAIT_SECONDS:-3000}"
deadline=$(( $(date +%s) + wait_seconds ))
identity="Gate verdict for pull_request #$PR_NUMBER head $HEAD_SHA base $BASE_SHA"
label_step="Require the $label label"
repo="repos/$GITHUB_REPOSITORY"

candidates="$(gh api "$repo/actions/workflows/$workflow/runs?event=pull_request&head_sha=$HEAD_SHA&per_page=100" \
  --jq "[.workflow_runs[] | select(.event == \"pull_request\" and .head_sha == \"$HEAD_SHA\" and .id < $GITHUB_RUN_ID)] | sort_by(.id) | reverse | .[].id")"

for run in $candidates; do
  # Attempt the re-run replaces; 0 while none was requested.
  rerun_from=0
  while :; do
    state="$(gh api "$repo/actions/runs/$run/jobs?per_page=100" \
      --jq "[.jobs[] | select(.name == \"$job\")] | last
        | if . == null then \"missing\"
          elif .status != \"completed\" or (.run_attempt // 1) <= $rerun_from then \"running\"
          else ([.steps[]? | select(.name == \"$identity\") | .conclusion] | last // \"absent\") as \$verdict
            | if (\$verdict == \"absent\" or \$verdict == \"skipped\")
                and ([.steps[]? | select(.name == \"$label_step\") | .conclusion] | last) == \"failure\"
              then \"unlabelled \(.run_attempt // 1)\" else \$verdict end
          end")"
    if [ "$state" = "missing" ] \
      && [ "$(gh api "$repo/actions/runs/$run" --jq .status)" = "completed" ]; then
      state="absent"
    fi
    case "$state" in
      success)
        echo "Run $run passed $job for pull request #$PR_NUMBER at head $HEAD_SHA, base $BASE_SHA."
        exit 0
        ;;
      failure|cancelled|timed_out)
        echo "::error::Run $run did not pass $job for this pull request state ($state): https://github.com/$GITHUB_REPOSITORY/actions/runs/$run"
        exit 1
        ;;
      unlabelled\ *)
        if [ "$rerun_from" -ne 0 ]; then
          echo "::error::Run $run still saw no $label label after its re-run: https://github.com/$GITHUB_REPOSITORY/actions/runs/$run"
          exit 1
        fi
        # The attempt that saw no label: a later one replaces it.
        rerun_from="${state#unlabelled }"
        if gh api -X POST "$repo/actions/runs/$run/rerun" >/dev/null; then
          echo "Run $run predates the $label label: re-running it."
        elif [ "$(gh api "$repo/actions/runs/$run" --jq '.run_attempt // 1')" -gt "$rerun_from" ]; then
          echo "Run $run is already being re-run."
        else
          echo "::error::Could not re-run $workflow run $run (a fork's token is read-only). A maintainer can use \"Re-run all jobs\" on https://github.com/$GITHUB_REPOSITORY/actions/runs/$run: it reads the labels live."
          exit 1
        fi
        ;;
      running|missing)
        if [ "$(date +%s)" -ge "$deadline" ]; then
          echo "::error::Run $run is still running $job after ${wait_seconds}s."
          exit 1
        fi
        echo "Waiting for run $run..."
        sleep 30
        ;;
      *)
        # Skipped or absent: that run tested another state.
        break
        ;;
    esac
  done
done

echo "::error::No earlier $workflow run tested pull request #$PR_NUMBER at head $HEAD_SHA, base $BASE_SHA. Push a commit, or close and reopen the pull request, to run it."
exit 1

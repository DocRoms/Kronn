#!/usr/bin/env bash
# Exits with the verdict an earlier run gave for this exact pull-request state.
#
# Adding or removing an unrelated label (or editing the title) starts a run
# that tests nothing. Its aggregate gate must not report that skip as a pass (a
# skipped required check counts as green) nor as a failure: it repeats the
# verdict of the run that tested the same pull request, head and base.
#
# Evidence is the gate's verdict step of an earlier run, whose name records the
# identity it tested: "Gate verdict for pull_request #<n> head <sha> base <sha>".
# A dispatch, another pull request, another base, or a run that itself only
# repeated a verdict never carries that step, so it is never accepted. Earlier
# means a lower run id, so two runs created in the same second still order.
#
# Usage: previous_gate_verdict.sh <workflow file> <gate job name>
# Environment: GH_TOKEN, GITHUB_REPOSITORY, GITHUB_RUN_ID, PR_NUMBER, HEAD_SHA,
# BASE_SHA.
set -euo pipefail

workflow="$1"
job="$2"
: "${GITHUB_REPOSITORY:?}" "${GITHUB_RUN_ID:?}" "${PR_NUMBER:?}" "${HEAD_SHA:?}" "${BASE_SHA:?}"
wait_seconds="${PREVIOUS_VERDICT_WAIT_SECONDS:-3000}"
deadline=$(( $(date +%s) + wait_seconds ))
identity="Gate verdict for pull_request #$PR_NUMBER head $HEAD_SHA base $BASE_SHA"

candidates="$(gh api "repos/$GITHUB_REPOSITORY/actions/workflows/$workflow/runs?event=pull_request&head_sha=$HEAD_SHA&per_page=100" \
  --jq "[.workflow_runs[] | select(.event == \"pull_request\" and .head_sha == \"$HEAD_SHA\" and .id < $GITHUB_RUN_ID)] | sort_by(.id) | reverse | .[].id")"

for run in $candidates; do
  while :; do
    state="$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run/jobs?per_page=100" \
      --jq "[.jobs[] | select(.name == \"$job\")] | last | if . == null then \"missing\" elif .status != \"completed\" then \"running\" else ([.steps[]? | select(.name == \"$identity\") | .conclusion] | last // \"absent\") end")"
    if [ "$state" = "missing" ] \
      && [ "$(gh api "repos/$GITHUB_REPOSITORY/actions/runs/$run" --jq .status)" = "completed" ]; then
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
      running|missing)
        if [ "$(date +%s)" -ge "$deadline" ]; then
          echo "::error::Run $run is still running $job after ${wait_seconds}s."
          exit 1
        fi
        echo "Waiting for run $run..."
        sleep 30
        ;;
      *)
        # Skipped or absent: that run tested another state, or only repeated.
        break
        ;;
    esac
  done
done

echo "::error::No earlier $workflow run tested pull request #$PR_NUMBER at head $HEAD_SHA, base $BASE_SHA. Push a commit, or remove and re-add the label, to run it."
exit 1

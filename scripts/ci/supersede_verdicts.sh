#!/usr/bin/env bash
# Marks the earlier check runs of a gate as superseded by this CI Verdict run.
#
# A pull request head collects several check runs named `ci-quality-gates` or
# `ci-build-gates`: one per label event (CI Verdict), one per CI Tests / CI
# Build run, one per attempt. GitHub keeps every check run it received, so a
# red or cancelled one from before the label stays listed beside the green one
# that replaced it. A check run cannot be deleted, but it can be updated: this
# sets the earlier ones to `neutral` with the title "Superseded by run <id>".
# The latest verdict is then the only success or failure of that gate on the
# head, and the history stays visible.
#
# A check run is superseded when ALL of these hold:
#   - it carries the gate's name and belongs to the same head SHA;
#   - it is older than this run's own gate job (a job id is its check run id,
#     and ids grow with time), whichever workflow published it: the verdict
#     repeats the CI Tests / CI Build result, so an older one adds nothing;
#   - it is completed and not already neutral (a gate never ends neutral, so
#     neutral means superseded already: the script is idempotent).
#
# An earlier CI Verdict run still winding down (cancelled by this one) has no
# completed check yet: it is awaited for a short while so it is not missed.
#
# Superseding is cosmetic: a read-only token (a fork) or an API error warns and
# exits 0, so it can never turn a verdict red.
#
# Usage: supersede_verdicts.sh <gate job name>
# Environment: GH_TOKEN, GITHUB_REPOSITORY, GITHUB_RUN_ID, HEAD_SHA; optional
# SUPERSEDE_WAIT_SECONDS (default 60) bounds the wait for cancelled runs.
set -uo pipefail

gate="$1"
: "${GITHUB_REPOSITORY:?}" "${GITHUB_RUN_ID:?}" "${HEAD_SHA:?}"
repo="repos/$GITHUB_REPOSITORY"
workflow="${VERDICT_WORKFLOW:-ci-verdict.yml}"
wait_seconds="${SUPERSEDE_WAIT_SECONDS:-60}"
deadline=$(( $(date +%s) + wait_seconds ))

if ! own="$(gh api "$repo/actions/runs/$GITHUB_RUN_ID/jobs?per_page=100" \
  --jq "[.jobs[] | select(.name == \"$gate\")] | last | .id // empty")" || [ -z "$own" ]; then
  echo "::warning::Could not find this run's $gate job; older verdicts are left as they are."
  exit 0
fi

# Earlier verdict runs of this head that have not finished (a cancel is not
# instant): their check runs only complete once they do.
while :; do
  if ! unfinished="$(gh api "$repo/actions/workflows/$workflow/runs?event=pull_request&head_sha=$HEAD_SHA&per_page=100" \
    --jq "[.workflow_runs[] | select(.head_sha == \"$HEAD_SHA\" and .id < $GITHUB_RUN_ID and (.status // \"completed\") != \"completed\") | .id] | length")"; then
    echo "::warning::Could not list earlier $workflow runs; older $gate verdicts are left as they are."
    exit 0
  fi
  if [ "$unfinished" = "0" ] || [ "$(date +%s)" -ge "$deadline" ]; then
    break
  fi
  echo "Waiting for $unfinished earlier $workflow run(s) to finish..."
  sleep 5
done

if ! stale="$(gh api "$repo/commits/$HEAD_SHA/check-runs?check_name=$gate&filter=all&per_page=100" \
  --jq "[.check_runs[]
        | select(.name == \"$gate\" and .status == \"completed\" and .conclusion != \"neutral\" and .id < $own)
        | .id] | .[]")"; then
  echo "::warning::Could not list the $gate check runs of $HEAD_SHA; older verdicts are left as they are."
  exit 0
fi

if [ -z "$stale" ]; then
  echo "No earlier $gate check run to supersede on $HEAD_SHA."
  exit 0
fi

for check in $stale; do
  if gh api -X PATCH "$repo/check-runs/$check" \
    -f status=completed -f conclusion=neutral \
    -f "output[title]=Superseded by run $GITHUB_RUN_ID" \
    -f "output[summary]=A later CI Verdict run reported $gate for the same head $HEAD_SHA: https://github.com/$GITHUB_REPOSITORY/actions/runs/$GITHUB_RUN_ID. Only that latest verdict counts." \
    >/dev/null; then
    echo "Check run $check marked as superseded by run $GITHUB_RUN_ID."
  else
    echo "::warning::Could not mark check run $check as superseded (a fork's token is read-only); it stays listed."
  fi
done
exit 0

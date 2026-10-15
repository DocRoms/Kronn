#!/usr/bin/env bash
# Marks the earlier CI Verdict check runs of a gate as superseded by this run.
#
# Every label event runs CI Verdict again on the same head SHA, and GitHub
# keeps each check run it received, so a red `ci-quality-gates` from before the
# label stays listed beside the green one that replaced it. A check run cannot
# be deleted, but it can be updated: this sets the earlier ones to `neutral`
# with the title "Superseded by run <id>". The latest verdict is then the only
# success or failure of that gate on the SHA, and the history stays visible.
#
# A check run is superseded when ALL of these hold:
#   - it carries the gate's name;
#   - its details_url points at a CI Verdict run for this head SHA whose id is
#     lower than this run's (CI Tests and CI Build publish the same gate names:
#     those are never touched);
#   - it is completed and not already neutral (a verdict job never ends
#     neutral, so neutral means superseded already: the script is idempotent).
#
# Superseding is cosmetic: a read-only token (a fork) or an API error warns and
# exits 0, so it can never turn a verdict red.
#
# Usage: supersede_verdicts.sh <gate job name>
# Environment: GH_TOKEN, GITHUB_REPOSITORY, GITHUB_RUN_ID, HEAD_SHA.
set -uo pipefail

gate="$1"
: "${GITHUB_REPOSITORY:?}" "${GITHUB_RUN_ID:?}" "${HEAD_SHA:?}"
repo="repos/$GITHUB_REPOSITORY"
workflow="${VERDICT_WORKFLOW:-ci-verdict.yml}"

if ! earlier_runs="$(gh api "$repo/actions/workflows/$workflow/runs?event=pull_request&head_sha=$HEAD_SHA&per_page=100" \
  --jq "[.workflow_runs[] | select(.head_sha == \"$HEAD_SHA\" and .id < $GITHUB_RUN_ID) | .id] | map(tostring) | join(\",\")")"; then
  echo "::warning::Could not list earlier $workflow runs; older $gate verdicts are left as they are."
  exit 0
fi
if [ -z "$earlier_runs" ]; then
  echo "No earlier $workflow run on $HEAD_SHA: nothing to supersede."
  exit 0
fi

if ! stale="$(gh api "$repo/commits/$HEAD_SHA/check-runs?check_name=$gate&filter=all&per_page=100" \
  --jq "[$earlier_runs] as \$runs
    | [.check_runs[]
        | select(.name == \"$gate\" and .status == \"completed\" and .conclusion != \"neutral\")
        | select((.details_url // \"\") | capture(\"/actions/runs/(?<run>[0-9]+)\") | .run | tonumber | IN(\$runs[]))
        | .id] | .[]")"; then
  echo "::warning::Could not list the $gate check runs of $HEAD_SHA; older verdicts are left as they are."
  exit 0
fi

if [ -z "$stale" ]; then
  echo "No earlier $gate verdict to supersede on $HEAD_SHA."
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

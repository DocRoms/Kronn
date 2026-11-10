#!/usr/bin/env bash
# Fails unless the pull request carries <label> NOW.
#
# The labels are read live (GET /pulls/<n>), never from the event payload: a
# re-run of a job replays the payload of the original event, so a label added or
# removed since then would be judged on a stale list (KT-1523, #507: a reopened
# pull request with both labels read as "missing label"). An unreadable list is
# not an absent label: it is retried, then reported as an API failure with its
# own exit code, so nobody is told to add a label that is there.
#
# Exit codes: 0 label present (or not a pull request), 1 label absent,
# 2 labels unreadable.
#
# Usage: require_label.sh <label> <message when the label is absent>
# Environment: GH_TOKEN, GITHUB_REPOSITORY, PR_NUMBER; optional EVENT_NAME (any
# value but "pull_request" has no label to require), REQUIRE_LABEL_ATTEMPTS
# (default 3), REQUIRE_LABEL_RETRY_SECONDS (default 5).
set -uo pipefail

label="$1"
message="$2"
: "${GITHUB_REPOSITORY:?}"

if [ "${EVENT_NAME:-pull_request}" != "pull_request" ]; then
  exit 0
fi
: "${PR_NUMBER:?}"

attempts="${REQUIRE_LABEL_ATTEMPTS:-3}"
if ! [[ "$attempts" =~ ^[1-9][0-9]*$ ]]; then
  attempts=3
fi

attempt=1
while :; do
  if labels="$(gh api "repos/$GITHUB_REPOSITORY/pulls/$PR_NUMBER" --jq '.labels[].name' 2>/dev/null)"; then
    if printf '%s\n' "$labels" | grep -qxF "$label"; then
      echo "Pull request #$PR_NUMBER carries the $label label."
      exit 0
    fi
    echo "::error::$message"
    exit 1
  fi
  if [ "$attempt" -ge "$attempts" ]; then
    echo "::error::Could not read the labels of pull request #$PR_NUMBER (GitHub API error): this is not a missing $label label. Re-run this job."
    exit 2
  fi
  attempt=$((attempt + 1))
  sleep "${REQUIRE_LABEL_RETRY_SECONDS:-5}"
done

#!/usr/bin/env bash
# Diff hygiene gate: whitespace errors and conflict markers in what a branch
# changes, never in the untouched history (generated files there predate it).
#
# Usage: scripts/check-diff.sh [base]
#   base  commit to compare HEAD with. Default: merge base of HEAD and
#         origin/main, or HEAD~1 when HEAD is on main already.
# Uncommitted changes (staged and unstaged) are checked too.
set -euo pipefail

base="${1:-}"
if [[ -z "$base" ]]; then
    base="$(git merge-base origin/main HEAD 2>/dev/null || true)"
    if [[ -z "$base" || "$base" == "$(git rev-parse HEAD)" ]]; then
        base="$(git rev-parse HEAD~1)"
    fi
fi

status=0
git diff --check "$base" HEAD || status=1
git diff --check --cached || status=1
git diff --check || status=1

if [[ $status -ne 0 ]]; then
    echo "check-diff: whitespace errors or conflict markers since ${base}" >&2
fi
exit "$status"

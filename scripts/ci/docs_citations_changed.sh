#!/usr/bin/env bash
# Lists the paths a pull request changes, for the doc-citation gate
# (backend/src/core/docs_citations.rs): it blocks only on the docs listed.
# The checkout is the pull request's merge commit, whose first parent is the
# base tip; checkout needs fetch-depth 2 to hold both parents.
#
# Usage: docs_citations_changed.sh <output file>
# Exports KRONN_DOCS_CITATIONS_CHANGED=<output file> through $GITHUB_ENV.
set -euo pipefail

out="$1"
: "${GITHUB_ENV:?}"
# Any other commit would diff the wrong range, and an empty list reads as a
# code-only pull request: fail instead.
if ! git rev-parse --verify --quiet 'HEAD^2' >/dev/null; then
  echo "::error::HEAD is not a pull request merge commit with both parents fetched (checkout fetch-depth: 2)."
  exit 1
fi
git diff --name-only 'HEAD^1' HEAD > "$out"
echo "KRONN_DOCS_CITATIONS_CHANGED=$out" >> "$GITHUB_ENV"
docs=$(grep -cE '^(backend/)?docs/' "$out" || true)
echo "$(wc -l < "$out" | tr -d ' ') changed path(s), ${docs} under docs/ or backend/docs/."

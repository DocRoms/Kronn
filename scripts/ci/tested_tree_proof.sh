#!/usr/bin/env bash
# Exits 0 when a pull-request run of a test workflow passed its aggregate on
# the exact tree being released. Desktop Build calls it at tag time: a proof
# replaces rerunning that workflow, no proof reruns it.
#
# A pull-request run tests the merge of its head into its base. When the base
# is an ancestor of the head, as the ruleset requires, that merge is the head's
# own tree. The aggregate's verdict step records what it tested, as in
# previous_gate_verdict.sh: "Gate verdict for pull_request #<n> head <sha>
# base <sha>". The newest run whose verdict covers the tree decides.
#
# Usage: tested_tree_proof.sh <workflow file> <gate job name>
# Environment: GH_TOKEN, GITHUB_REPOSITORY, RELEASE_TREE.
set -euo pipefail

workflow="$1"
job="$2"
: "${GITHUB_REPOSITORY:?}" "${RELEASE_TREE:?}"
repo="repos/$GITHUB_REPOSITORY"
# "<head>=<tree> " pairs already read: several runs share a head.
trees=" "

# Sets `tree` to the head's tree, or "none" when the API cannot read it.
read_tree() {
  local head="$1" entry
  entry="${trees##* "$head"=}"
  if [ "$entry" != "$trees" ]; then
    tree="${entry%% *}"
    return
  fi
  tree="$(gh api "$repo/git/commits/$head" --jq .tree.sha 2>/dev/null || true)"
  tree="${tree:-none}"
  trees="$trees$head=$tree "
}

runs="$(gh api "$repo/actions/workflows/$workflow/runs?event=pull_request&status=completed&per_page=100" \
  --jq '.workflow_runs | sort_by(.id) | reverse | .[] | select(.event == "pull_request") | "\(.id):\(.head_sha)"')"

for entry in $runs; do
  run="${entry%%:*}"
  head="${entry#*:}"
  read_tree "$head"
  [ "$tree" = "$RELEASE_TREE" ] || continue
  verdict="$(gh api "$repo/actions/runs/$run/jobs?per_page=100" \
    --jq "[.jobs[] | select(.name == \"$job\")] | last
      | if . == null or .status != \"completed\" then empty
        else [.steps[]? | select(.name | test(\"^Gate verdict for pull_request #[0-9]+ head $head base [0-9a-f]{40}$\"))]
          | last | if . == null then empty else \"\(.conclusion) \(.name)\" end
        end")"
  conclusion="${verdict%% *}"
  # No verdict, or a skipped one: that run saw no label.
  if [ -z "$verdict" ] || [ "$conclusion" = "skipped" ]; then
    continue
  fi
  base="${verdict##* base }"
  ancestry="$(gh api "$repo/compare/$base...$head" --jq .status)"
  if [ "$ancestry" != "ahead" ] && [ "$ancestry" != "identical" ]; then
    echo "Run $run: base $base is not an ancestor of head $head, so it tested another tree." >&2
    continue
  fi
  if [ "$conclusion" != "success" ]; then
    echo "Run $run, the newest verdict on tree $RELEASE_TREE, did not pass $job ($conclusion)." >&2
    exit 1
  fi
  echo "Run $run passed $job on tree $RELEASE_TREE (${verdict#* for }): https://github.com/$GITHUB_REPOSITORY/actions/runs/$run"
  exit 0
done

echo "No completed pull-request run of $workflow passed $job on tree $RELEASE_TREE." >&2
exit 1

#!/usr/bin/env bats
# scripts/ci/rearm_verdict.sh against a stubbed `gh` (tests/bats/ci_stubs.bash),
# and the security shape of the workflow that runs it.

load test_helper
load ci_stubs

SHA="0123456789abcdef0123456789abcdef01234567"
WORKFLOW="${BATS_TEST_DIRNAME}/../../.github/workflows/ci-verdict-rearm.yml"

setup() {
    install_ci_stubs
    export GITHUB_REPOSITORY=o/r UPSTREAM_NAME="CI Build" HEAD_SHA="$SHA" REARM_WAIT_SECONDS=0
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# verdict_runs <run id>...: the CI Verdict runs of the head.
verdict_runs() {
    local items="" id
    for id in "$@"; do
        items="${items:+$items,}{\"id\":$id,\"head_sha\":\"$HEAD_SHA\"}"
    done
    printf '{"workflow_runs":[%s]}' "$items" > "$FIXTURES/actions_workflows_ci-verdict.yml_runs.json"
}

# verdict_run <run id> <status>
verdict_run() {
    printf '{"status":"%s"}' "$2" > "$FIXTURES/actions_runs_$1.json"
}

# gate_job <run id> <job id> <gate> <status> <conclusion> [step conclusion]
gate_job() {
    printf '{"jobs":[{"id":%s,"name":"%s","status":"%s","conclusion":%s,"steps":[{"name":"Repeat","conclusion":"%s"}]},{"id":999,"name":"other","status":"completed","conclusion":"cancelled","steps":[]}]}' \
        "$2" "$3" "$4" "$5" "${6:-success}" > "$FIXTURES/actions_runs_$1_jobs.json"
}

rearm() {
    run "${PROJECT_ROOT}/scripts/ci/rearm_verdict.sh"
}

@test "rearm: a gate job that timed out is re-run once its upstream run completed" {
    verdict_runs 500
    verdict_run 500 completed
    gate_job 500 555 ci-build-gates completed '"cancelled"' cancelled
    rearm
    assert_success
    posted "repos/o/r/actions/jobs/555/rerun"
    assert_output --partial "Re-ran ci-build-gates of CI Verdict run 500"
}

@test "rearm: CI Tests re-runs the quality gate, not the build gate" {
    export UPSTREAM_NAME="CI Tests"
    verdict_runs 500
    verdict_run 500 completed
    printf '{"jobs":[{"id":11,"name":"ci-build-gates","status":"completed","conclusion":"cancelled","steps":[]},{"id":22,"name":"ci-quality-gates","status":"completed","conclusion":"cancelled","steps":[]}]}' \
        > "$FIXTURES/actions_runs_500_jobs.json"
    rearm
    assert_success
    posted "repos/o/r/actions/jobs/22/rerun"
    ! posted "repos/o/r/actions/jobs/11/rerun"
}

@test "rearm: a job cut off but reported failed (cancelled step) is re-run" {
    verdict_runs 500
    verdict_run 500 completed
    gate_job 500 555 ci-build-gates completed '"failure"' cancelled
    rearm
    assert_success
    posted "repos/o/r/actions/jobs/555/rerun"
}

@test "rearm: a gate that passed is left alone" {
    verdict_runs 500
    verdict_run 500 completed
    gate_job 500 555 ci-build-gates completed '"success"'
    rearm
    assert_success
    assert_output --partial "nothing to re-arm"
    no_post
}

@test "rearm: a gate that really failed is left alone" {
    verdict_runs 500
    verdict_run 500 completed
    gate_job 500 555 ci-build-gates completed '"failure"' failure
    rearm
    assert_success
    no_post
}

@test "rearm: only the newest verdict run of the head is touched" {
    verdict_runs 400 500
    verdict_run 500 completed
    gate_job 500 555 ci-build-gates completed '"cancelled"' cancelled
    # Run 400 has no fixture: reading it would fail.
    rearm
    assert_success
    posted "repos/o/r/actions/jobs/555/rerun"
    ! grep -q 'actions/runs/400' "$FIXTURES/calls.log"
}

@test "rearm: a verdict run still going is awaited, then its cut-off job is re-run" {
    export REARM_WAIT_SECONDS=600
    verdict_runs 500
    verdict_run 500 in_progress
    gate_job 500 555 ci-build-gates completed '"cancelled"' cancelled
    on_sleep "printf '{\"status\":\"completed\"}' > \"$FIXTURES/actions_runs_500.json\""
    rearm
    assert_success
    [ "$(sleeps)" -eq 1 ]
    posted "repos/o/r/actions/jobs/555/rerun"
}

@test "rearm: a sibling gate that keeps polling does not make it re-run anything" {
    export REARM_WAIT_SECONDS=600
    verdict_runs 500
    verdict_run 500 in_progress
    gate_job 500 555 ci-build-gates in_progress null
    # The running job finishes on its own, passing, before the run completes.
    on_sleep "printf '{\"status\":\"completed\"}' > \"$FIXTURES/actions_runs_500.json\"; sed -i.bak 's/in_progress/completed/; s/null/\"success\"/' \"$FIXTURES/actions_runs_500_jobs.json\""
    rearm
    assert_success
    no_post
}

@test "rearm: a verdict run that never completes ends with a warning, not a failure" {
    verdict_runs 500
    verdict_run 500 in_progress
    rearm
    assert_success
    assert_output --partial "Gave up re-arming ci-build-gates of run 500"
    no_post
}

@test "rearm: a refused re-run warns by hand and still exits 0" {
    verdict_runs 500
    verdict_run 500 completed
    gate_job 500 555 ci-build-gates completed '"cancelled"' cancelled
    POST_REFUSED=rerun rearm
    assert_success
    assert_output --partial "Could not re-run ci-build-gates of run 500"
    assert_output --partial "Re-run CI Verdict by hand"
}

@test "rearm: a head without a verdict run has nothing to re-arm" {
    verdict_runs
    rearm
    assert_success
    assert_output --partial "nothing to re-arm"
    no_post
}

@test "rearm: a workflow without a gate asks nothing of GitHub" {
    UPSTREAM_NAME="Docker Images" rearm
    assert_success
    [ ! -e "$FIXTURES/calls.log" ]
}

@test "rearm: a head that is not a commit SHA asks nothing of GitHub" {
    HEAD_SHA='x"; touch pwned; "' rearm
    assert_success
    [ ! -e "$FIXTURES/calls.log" ]
}

# The workflow gets a write token whatever the pull request is: its shape is
# part of the contract.

@test "rearm workflow: starts on workflow_run only, never on pull_request events" {
    run grep -nE '^on:|^  [a-z_]+:$' "$WORKFLOW"
    assert_output --partial "workflow_run:"
    refute_output --partial "pull_request"
    refute_output --partial "push:"
}

@test "rearm workflow: minimal permissions, nothing at the top level" {
    grep -qx 'permissions: {}' "$WORKFLOW"
    run awk '/^    permissions:/{f=1; next} f && /^      /{print; next} f{exit}' "$WORKFLOW"
    assert_output "      actions: write
      contents: read"
}

@test "rearm workflow: checks out the default branch only, without credentials" {
    grep -q 'ref: ${{ github.event.repository.default_branch }}' "$WORKFLOW"
    grep -q 'persist-credentials: false' "$WORKFLOW"
    # No other ref is ever named.
    [ "$(grep -c '^ *ref:' "$WORKFLOW")" -eq 1 ]
}

@test "rearm workflow: pull-request controlled values reach the shell through env only" {
    # Every use of the event payload is a job guard or an env assignment.
    run grep -n 'github\.event\.workflow_run' "$WORKFLOW"
    local line
    while IFS= read -r line; do
        [[ "$line" =~ :\ +(if:|UPSTREAM_NAME:|HEAD_SHA:) ]] || { echo "unexpected use: $line"; return 1; }
    done <<<"$output"
    # And the single script it runs is ours, with no inline expression.
    [ "$(grep -c '^ *run:' "$WORKFLOW")" -eq 1 ]
    grep -qx '        run: scripts/ci/rearm_verdict.sh' "$WORKFLOW"
}

no_post() {
    ! grep -q '^POST' "$FIXTURES/calls.log" 2>/dev/null
}

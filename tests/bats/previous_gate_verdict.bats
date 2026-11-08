#!/usr/bin/env bats
# Runs the real CI Verdict script against a stubbed `gh` that serves fixture
# API responses through the caller's own --jq filter (tests/bats/ci_stubs.bash).

load test_helper
load ci_stubs

ID="Gate verdict for pull_request #7 head h7 base b7"

setup() {
    install_ci_stubs
    export GITHUB_REPOSITORY=o/r GITHUB_RUN_ID=1000 PR_NUMBER=7 HEAD_SHA=h7 BASE_SHA=b7
    head_is h7
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

runs() {
    local items="" id
    for id in "$@"; do
        items="${items:+$items,}{\"id\":$id,\"event\":\"pull_request\",\"head_sha\":\"h7\"}"
    done
    printf '{"workflow_runs":[%s]}' "$items" > "$FIXTURES/actions_workflows_ci-build.yml_runs.json"
}

# jobs <run id> <gate verdict step conclusion> [cancelled job name]
jobs() {
    local extra=""
    [ -n "${3:-}" ] && extra=",{\"name\":\"$3\",\"status\":\"completed\",\"conclusion\":\"cancelled\",\"steps\":[]}"
    printf '{"jobs":[{"name":"ci-build-gates","status":"completed","run_attempt":1,"conclusion":"%s","steps":[{"name":"%s","conclusion":"%s"}]}%s]}' \
        "$2" "$ID" "$2" "$extra" > "$FIXTURES/actions_runs_$1_jobs.json"
    printf '{"status":"completed","run_attempt":1}' > "$FIXTURES/actions_runs_$1.json"
}

# head_is <sha>: the head the pull request has now.
head_is() {
    printf '{"head":{"sha":"%s"}}' "$1" > "$FIXTURES/pulls_7.json"
}

# running <run id>: the gate job has not finished.
running() {
    printf '{"jobs":[{"name":"ci-build-gates","status":"in_progress","run_attempt":1,"conclusion":null,"steps":[]}]}' \
        > "$FIXTURES/actions_runs_$1_jobs.json"
    printf '{"status":"in_progress","run_attempt":1}' > "$FIXTURES/actions_runs_$1.json"
}

# finishes <run id> <conclusion>: the run is still going, then completes while the script sleeps.
finishes() {
    jobs "$1" "$2"
    mv "$FIXTURES/actions_runs_$1_jobs.json" "$FIXTURES/done_jobs.json"
    running "$1"
    on_sleep "cp \"$FIXTURES/done_jobs.json\" \"$FIXTURES/actions_runs_$1_jobs.json\""
}

# Nothing was asked of GitHub but reads.
no_post() {
    ! grep -q '^POST' "$FIXTURES/calls.log"
}

verdict() {
    run "${PROJECT_ROOT}/scripts/ci/previous_gate_verdict.sh" ci-build.yml ci-build-gates ci-build
}

@test "verdict: a cancelled gate defers to an earlier run that passed" {
    runs 200 100
    jobs 200 cancelled
    jobs 100 success
    verdict
    assert_success
    assert_output --partial "Run 100 passed ci-build-gates"
}

@test "verdict: a gate failed by a cancelled job names that job" {
    runs 200
    jobs 200 failure "test-backend-portability (macos-latest)"
    verdict
    assert_failure
    assert_output --partial "cancelled, not failed: test-backend-portability (macos-latest)"
}

@test "verdict: a gate cancelled with no earlier verdict asks for a re-run" {
    runs 200
    jobs 200 cancelled
    verdict
    assert_failure
    assert_output --partial "Run 200 was cancelled before ci-build-gates decided"
}

@test "verdict: a real failure is repeated as is" {
    runs 200 100
    jobs 200 failure
    jobs 100 success
    verdict
    assert_failure
    assert_output --partial "did not pass ci-build-gates for this pull request state (failure)"
}

@test "verdict: a run started after the verdict is accepted (label event first)" {
    # Run ids above GITHUB_RUN_ID (1000): the label event beat the CI run.
    runs 1002
    jobs 1002 success
    verdict
    assert_success
    assert_output --partial "Run 1002 passed ci-build-gates"
}

@test "verdict: the newest run decides, not the oldest" {
    runs 1002 100
    jobs 1002 failure
    jobs 100 success
    verdict
    assert_failure
    assert_output --partial "Run 1002 did not pass ci-build-gates"
}

@test "verdict: no run yet is awaited, then reported" {
    runs
    PREVIOUS_VERDICT_DISCOVERY_SECONDS=0 verdict
    assert_failure
    assert_output --partial "No ci-build.yml run tested pull request #7 at head h7, base b7"
}

@test "verdict: upstream still running after the wait is held, then repeated green" {
    runs 1001
    finishes 1001 success
    PREVIOUS_VERDICT_WAIT_SECONDS=0 verdict
    assert_success
    assert_output --partial "still running ci-build-gates after 0s"
    assert_output --partial "Run 1001 passed ci-build-gates"
    # Said once, not at every poll; nothing was cancelled or re-run.
    [ "$(grep -c 'still running ci-build-gates' <<<"$output")" -eq 1 ]
    no_post
}

@test "verdict: upstream still running after the wait, then failed, is repeated red" {
    runs 1001
    finishes 1001 failure
    PREVIOUS_VERDICT_WAIT_SECONDS=0 verdict
    assert_failure 1
    assert_output --partial "still running ci-build-gates after 0s"
    assert_output --partial "Run 1001 did not pass ci-build-gates for this pull request state (failure)"
    no_post
}

@test "verdict: upstream still running when the job is cut off never exits red or green and never cancels" {
    runs 1001
    running 1001
    # The job timeout of the real run: GitHub ends the step from outside.
    on_sleep '[ "$(wc -l < "$FIXTURES/sleeps")" -lt 4 ] || kill -TERM $PPID'
    # A fork's token could not cancel or re-run anything either way.
    POST_REFUSED="cancel rerun" PREVIOUS_VERDICT_WAIT_SECONDS=0 verdict
    [ "$status" -eq 143 ]
    assert_output --partial "This check stays pending"
    refute_output --partial "::error::"
    no_post
    [ "$(sleeps)" -eq 4 ]
}

@test "verdict: a head the pull request moved past is never repeated green" {
    runs 1001
    jobs 1001 success
    head_is h8
    verdict
    assert_failure 1
    refute_output --partial "passed"
    assert_output --partial "moved past head h7"
    # Not even the upstream run was read.
    ! grep -q 'actions/runs/1001' "$FIXTURES/calls.log"
}

@test "verdict: an obsolete head cancels its own run and stops holding a runner" {
    runs 1001
    running 1001
    head_is h8
    # The cancel stops the step during its grace period.
    on_sleep 'kill -TERM $PPID'
    PREVIOUS_VERDICT_CANCEL_GRACE_SECONDS=7 verdict
    [ "$status" -eq 143 ]
    posted "repos/o/r/actions/runs/1000/cancel"
    grep -qx 'sleep 7' "$FIXTURES/calls.log"
}

@test "verdict: an obsolete head with a refused cancel (fork token) exits red at once" {
    runs 1001
    running 1001
    head_is h8
    POST_REFUSED=cancel verdict
    assert_failure 1
    assert_output --partial "Could not cancel run 1000 (a fork's token is read-only)"
    [ "$(sleeps)" -eq 0 ]
}

@test "verdict: an obsolete head whose cancel outlasts the grace period exits red, never green" {
    runs 1001
    running 1001
    head_is h8
    PREVIOUS_VERDICT_CANCEL_GRACE_SECONDS=7 verdict
    assert_failure 1
    assert_output --partial "kept running after its cancel"
    grep -qx 'sleep 7' "$FIXTURES/calls.log"
}

@test "verdict: a head that moves while the upstream run is running stops the wait" {
    runs 1001
    running 1001
    # First sleep: a push lands. Second sleep (the cancel's grace period): cut off.
    on_sleep 'if [ -f "$FIXTURES/moved" ]; then kill -TERM $PPID; else printf "{\"head\":{\"sha\":\"h8\"}}" > "$FIXTURES/pulls_7.json"; touch "$FIXTURES/moved"; fi'
    verdict
    [ "$status" -eq 143 ]
    assert_output --partial "Waiting for run 1001"
    assert_output --partial "moved past head h7"
    posted "repos/o/r/actions/runs/1000/cancel"
}

@test "verdict: an unreadable pull request head does not stop the verdict" {
    runs 1001
    jobs 1001 success
    rm "$FIXTURES/pulls_7.json"
    verdict
    assert_success
    assert_output --partial "Run 1001 passed ci-build-gates"
}

@test "verdict: a run that appears while waiting is picked up" {
    runs
    # The first listing is empty; the stubbed sleep publishes the run.
    printf '#!/usr/bin/env bash\ncp "%s/late_runs.json" "%s/actions_workflows_ci-build.yml_runs.json"\n' \
        "$FIXTURES" "$FIXTURES" > "$TEST_TMPDIR/bin/sleep"
    chmod +x "$TEST_TMPDIR/bin/sleep"
    runs 1001
    mv "$FIXTURES/actions_workflows_ci-build.yml_runs.json" "$FIXTURES/late_runs.json"
    runs
    jobs 1001 success
    verdict
    assert_success
    assert_output --partial "waiting for it to start"
    assert_output --partial "Run 1001 passed ci-build-gates"
}

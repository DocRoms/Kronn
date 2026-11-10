#!/usr/bin/env bats
# scripts/ci/require_label.sh against a stubbed `gh` (tests/bats/ci_stubs.bash),
# chained with previous_gate_verdict.sh for the reopened-pull-request case, and
# the way the three CI workflows use them (KT-1523, #507 of 2026-11-09).

load test_helper
load ci_stubs

WORKFLOWS="${BATS_TEST_DIRNAME}/../../.github/workflows"
MESSAGE="Add the ci-build label before merging."

setup() {
    install_ci_stubs
    export GITHUB_REPOSITORY=o/r PR_NUMBER=7 GITHUB_RUN_ID=1000 HEAD_SHA=h7 BASE_SHA=b7
    export REQUIRE_LABEL_RETRY_SECONDS=1
    labels_are ci-test ci-build
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# labels_are <label>...: the labels the pull request carries right now.
labels_are() {
    local items="" name
    for name in "$@"; do
        items="${items:+$items,}{\"name\":\"$name\"}"
    done
    printf '{"head":{"sha":"h7"},"labels":[%s]}' "$items" > "$FIXTURES/pulls_7.json"
}

require() {
    run "${PROJECT_ROOT}/scripts/ci/require_label.sh" "$1" "$MESSAGE"
}

# ── the guard ────────────────────────────────────────────────────────────────

@test "require_label: a label the pull request carries passes" {
    require ci-build
    assert_success
    assert_output --partial "carries the ci-build label"
}

@test "require_label: an absent label fails with the message, exit 1" {
    labels_are ci-test
    require ci-build
    assert_failure 1
    assert_output --partial "::error::$MESSAGE"
}

@test "require_label: the label is matched whole, not as a substring or a pattern" {
    labels_are ci-build-extra "xci-build" "ci.build"
    require ci-build
    assert_failure 1
    require "ci.build"
    assert_success
}

@test "require_label: a re-run after the label was added sees it (stale payload ignored)" {
    # The replayed event payload still lists no label: the script never reads it.
    export GITHUB_EVENT_PATH="$TEST_TMPDIR/event.json"
    printf '{"pull_request":{"labels":[]}}' > "$GITHUB_EVENT_PATH"
    labels_are ci-test
    require ci-build
    assert_failure 1
    labels_are ci-test ci-build
    require ci-build
    assert_success
}

@test "require_label: a label removed since the original event is not reported present" {
    export GITHUB_EVENT_PATH="$TEST_TMPDIR/event.json"
    printf '{"pull_request":{"labels":[{"name":"ci-build"}]}}' > "$GITHUB_EVENT_PATH"
    labels_are ci-test
    require ci-build
    assert_failure 1
}

@test "require_label: an unreadable list is retried, then reported as an API failure, never as a missing label" {
    rm "$FIXTURES/pulls_7.json"
    require ci-build
    assert_failure 2
    assert_output --partial "Could not read the labels of pull request #7"
    assert_output --partial "not a missing ci-build label"
    refute_output --partial "::error::$MESSAGE"
    [ "$(sleeps)" -eq 2 ]
}

@test "require_label: a list unreadable once, then readable, passes" {
    rm "$FIXTURES/pulls_7.json"
    on_sleep "printf '{\"labels\":[{\"name\":\"ci-build\"}]}' > \"$FIXTURES/pulls_7.json\""
    require ci-build
    assert_success
    [ "$(sleeps)" -eq 1 ]
}

@test "require_label: a push or a dispatch has no label to require and asks nothing" {
    EVENT_NAME=push run "${PROJECT_ROOT}/scripts/ci/require_label.sh" ci-build "$MESSAGE"
    assert_success
    [ ! -e "$FIXTURES/calls.log" ]
    EVENT_NAME=workflow_dispatch run "${PROJECT_ROOT}/scripts/ci/require_label.sh" ci-build "$MESSAGE"
    assert_success
    [ ! -e "$FIXTURES/calls.log" ]
}

@test "require_label: a pull_request event is checked" {
    labels_are ci-test
    EVENT_NAME=pull_request run "${PROJECT_ROOT}/scripts/ci/require_label.sh" ci-build "$MESSAGE"
    assert_failure 1
}

# ── a reopened pull request with both labels ─────────────────────────────────

# upstream <workflow> <gate> <conclusion>: a finished run 1002 whose verdict step
# recorded this pull request state.
upstream() {
    printf '{"workflow_runs":[{"id":1002,"event":"pull_request","head_sha":"h7"}]}' \
        > "$FIXTURES/actions_workflows_$1_runs.json"
    printf '{"jobs":[{"name":"%s","status":"completed","run_attempt":1,"conclusion":"%s","steps":[{"name":"Gate verdict for pull_request #7 head h7 base b7","conclusion":"%s"}]}]}' \
        "$2" "$3" "$3" > "$FIXTURES/actions_runs_1002_jobs.json"
    printf '{"status":"completed","run_attempt":1}' > "$FIXTURES/actions_runs_1002.json"
}

# One verdict job: the guard, then the upstream verdict, as ci-verdict.yml runs them.
verdict_job() { # <label> <workflow> <gate>
    run bash -c '
        "$1/scripts/ci/require_label.sh" "$2" "missing $2" || exit $?
        "$1/scripts/ci/previous_gate_verdict.sh" "$3" "$4" "$2"' _ "$PROJECT_ROOT" "$1" "$2" "$3"
}

@test "reopened pull request with both labels: the gates evaluate the upstream results, green when they passed" {
    # The verdict is replayed after a close/reopen; the event payload listed
    # nothing useful, the live labels list both.
    export GITHUB_EVENT_PATH="$TEST_TMPDIR/event.json"
    printf '{"pull_request":{"labels":[]}}' > "$GITHUB_EVENT_PATH"
    upstream ci-test.yml ci-quality-gates success
    verdict_job ci-test ci-test.yml ci-quality-gates
    assert_success
    assert_output --partial "Run 1002 passed ci-quality-gates"
    upstream ci-build.yml ci-build-gates success
    verdict_job ci-build ci-build.yml ci-build-gates
    assert_success
    assert_output --partial "Run 1002 passed ci-build-gates"
    no_post
}

@test "reopened pull request with both labels: a real upstream failure is still repeated red" {
    upstream ci-build.yml ci-build-gates failure
    verdict_job ci-build ci-build.yml ci-build-gates
    assert_failure 1
    assert_output --partial "did not pass ci-build-gates for this pull request state (failure)"
}

@test "a verdict job without the label is red before it looks at any upstream run" {
    labels_are ci-test
    upstream ci-build.yml ci-build-gates success
    verdict_job ci-build ci-build.yml ci-build-gates
    assert_failure 1
    assert_output --partial "missing ci-build"
    no_call "actions/workflows/ci-build.yml/runs"
}

# ── how the workflows use it ─────────────────────────────────────────────────

# step <file> <step name>: the lines of that step, up to the next step.
step() {
    awk -v name="      - name: $2" '
        $0 == name { found = 1; print; next }
        found && /^      - / { exit }
        found && /^  [a-z]/ { exit }
        found { print }' "$WORKFLOWS/$1"
}

@test "ci-verdict workflow: the label guards read live labels and are never conditional" {
    run grep -n 'pull_request\.labels' "$WORKFLOWS/ci-verdict.yml"
    assert_failure 1
    local label guard
    for label in ci-test ci-build; do
        guard="$(step ci-verdict.yml "Require the $label label")"
        [[ "$guard" == *"scripts/ci/require_label.sh $label "* ]] || { echo "no live guard for $label"; return 1; }
        [[ "$guard" != *"if:"* ]] || { echo "the $label guard is conditional"; return 1; }
    done
    # It reads the labels with the pull-requests permission, in both jobs.
    [ "$(grep -c 'pull-requests: read' "$WORKFLOWS/ci-verdict.yml")" -eq 2 ]
}

@test "ci-test and ci-build workflows: the require-label job reads live labels through the script" {
    grep -q 'scripts/ci/require_label.sh ci-test ' "$WORKFLOWS/ci-test.yml"
    grep -q 'scripts/ci/require_label.sh ci-build ' "$WORKFLOWS/ci-build.yml"
    run grep -n 'pull_request\.labels' "$WORKFLOWS/ci-test.yml" "$WORKFLOWS/ci-build.yml"
    assert_failure 1
}

@test "ci-test and ci-build workflows: only a label check that failed means 'label missing'" {
    local file label needs guard notrun
    for file in "ci-test.yml ci-test require-ci-label" "ci-build.yml ci-build require-ci-build-label"; do
        set -- $file
        guard="$(step "$1" "Require the $2 label")"
        [[ "$guard" == *"if: needs.$3.result == 'failure'"* ]] || { echo "$1: the label guard does not test result == failure"; return 1; }
        notrun="$(step "$1" "Fail when the label check did not run")"
        [[ "$notrun" == *"needs.$3.result != 'success' && needs.$3.result != 'failure'"* ]] || { echo "$1: no step for a label check that did not run"; return 1; }
        # The verdict step is still gated on the check's own success output.
        grep -q "if: needs.$3.outputs.run == 'true'" "$WORKFLOWS/$1"
        # A skipped or cancelled check is never read as a missing label.
        [[ "$guard" != *"outputs.run != 'true'"* ]] || { echo "$1: a skipped check is read as a missing label"; return 1; }
    done
}

@test "ci-test and ci-build workflows: a label check that did not run fails the gate without blaming the label" {
    local file notrun
    for file in ci-test.yml ci-build.yml; do
        notrun="$(step "$file" "Fail when the label check did not run")"
        [[ "$notrun" == *"exit 1"* ]] || { echo "$file: a check that did not run must fail the gate"; return 1; }
        [[ "$notrun" == *"not a missing label"* ]] || { echo "$file: the message blames the label"; return 1; }
    done
}

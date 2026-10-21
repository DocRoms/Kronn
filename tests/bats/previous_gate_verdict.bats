#!/usr/bin/env bats
# Runs the real CI Verdict script against a stubbed `gh` that serves fixture
# API responses through the caller's own --jq filter.

load test_helper

ID="Gate verdict for pull_request #7 head h7 base b7"

setup() {
    TEST_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/kronn-verdict-XXXXXX")"
    mkdir -p "$TEST_TMPDIR/bin" "$TEST_TMPDIR/fx"
    cat > "$TEST_TMPDIR/bin/gh" <<'GH'
#!/usr/bin/env bash
shift
method=GET url="" filter="."
while [ $# -gt 0 ]; do
    case "$1" in
        -X) method="$2"; shift 2 ;;
        --jq) filter="$2"; shift 2 ;;
        *) url="$1"; shift ;;
    esac
done
echo "$method $url" >> "$FIXTURES/calls.log"
[ "$method" = POST ] && exit 0
key="$(printf '%s' "$url" | sed -E 's#^repos/[^/]+/[^/]+/##; s#\?.*##; s#/#_#g')"
jq -r "$filter" "$FIXTURES/$key.json"
GH
    chmod +x "$TEST_TMPDIR/bin/gh"
    export PATH="$TEST_TMPDIR/bin:$PATH" FIXTURES="$TEST_TMPDIR/fx"
    export GITHUB_REPOSITORY=o/r GITHUB_RUN_ID=1000 PR_NUMBER=7 HEAD_SHA=h7 BASE_SHA=b7
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

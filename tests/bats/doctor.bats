#!/usr/bin/env bats
# Regression coverage for the repository-perimeter preflight (#196).

load test_helper

setup() {
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-doctor-XXXXXX)"
    export HOME="$TEST_TMPDIR/home"
    export KRONN_SOURCE_ONLY=1
    mkdir -p "$HOME" "$HOME/.config/kronn"
    source "$PROJECT_ROOT/kronn"
    export KRONN_CONFIG_DIR="$HOME/.config/kronn"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

@test "_path_under_root rejects a sibling with the same prefix" {
    run _path_under_root "/repos-other/app" "/repos"
    assert_failure
    run _path_under_root "/repos/app" "/repos"
    assert_success
}

@test "doctor reports a visible read-only project and missing secrets together" {
    local primary="$TEST_TMPDIR/repos"
    local outside="$TEST_TMPDIR/Euronews_Front"
    mkdir -p "$primary" "$outside"
    printf '{"token":"${GITHUB_PERSONAL_ACCESS_TOKEN}"}\n' > "$outside/.mcp.json.example"
    export KRONN_REPOS_DIR="$primary"
    export KRONN_EXTRA_REPOS=""
    _doctor_project_paths() { printf '%s\n' "$outside"; }
    _doctor_host_sync_labels() { return 0; }

    run cmd_doctor
    assert_failure
    assert_output --partial "visible but read-only"
    assert_output --partial "add its parent to KRONN_EXTRA_REPOS and restart Kronn"
    assert_output --partial "Secrets store missing"
    assert_output --partial "missing referenced secrets GITHUB_PERSONAL_ACCESS_TOKEN"
}

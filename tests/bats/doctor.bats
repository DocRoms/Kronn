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

@test "doctor from a service release CLI reports no repositories directory, not the release" {
    local project="$TEST_TMPDIR/somewhere/app"
    mkdir -p "$project" "$TEST_TMPDIR/svc/current/cli"
    KRONN_DIR="$TEST_TMPDIR/svc/current/cli"   # no Makefile, no cli.toml
    export KRONN_REPOS_DIR="" KRONN_EXTRA_REPOS=""
    _kronn_env_value() { return 1; }
    _doctor_project_paths() { printf '%s\n' "$project"; }
    _doctor_host_sync_labels() { return 0; }

    run cmd_doctor
    assert_failure
    assert_output --partial "No repositories directory configured"
    refute_output --partial "Primary rw directory"
    refute_output --partial "$TEST_TMPDIR/svc/current"
    # No empty root: the project is neither "inside" nor blamed on a root.
    refute_output --partial "inside rw perimeter"
}

@test "mcp sync from a service release CLI says no directory is configured and scans nothing" {
    mkdir -p "$TEST_TMPDIR/svc/current/cli"
    KRONN_DIR="$TEST_TMPDIR/svc/current/cli"
    scan_repos() { echo "scanned $1" >> "$TEST_TMPDIR/calls"; }
    sync_mcp_all() { echo "synced" >> "$TEST_TMPDIR/calls"; }
    run cmd_mcp sync
    assert_failure
    assert_output --partial "No repositories directory configured"
    [ ! -e "$TEST_TMPDIR/calls" ]
}

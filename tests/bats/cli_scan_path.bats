#!/usr/bin/env bats
# ─── CLI mode from a service release CLI (current/cli) ──────────────────────
# KRONN_DIR has no Makefile and no repositories directory is saved: CLI mode
# must ask before scanning, and status must not scan at all, never falling
# back to scan_repos "" (which scans the current directory).

load test_helper

setup() {
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-scan-path-XXXXXX)"
    export HOME="$TEST_TMPDIR/home"
    export KRONN_SOURCE_ONLY=1
    mkdir -p "$HOME/.config/kronn" "$HOME/Repositories" "$TEST_TMPDIR/svc/current/cli"
    source "$PROJECT_ROOT/kronn"
    source "$PROJECT_ROOT/tests/bats/bats-support/src/error.bash"
    export KRONN_CONFIG_DIR="$HOME/.config/kronn"
    KRONN_DIR="$TEST_TMPDIR/svc/current/cli"
    CALLS="$TEST_TMPDIR/calls"
    scan_repos() { echo "scan $1" >> "$CALLS"; REPO_PATHS=(); REPO_NAMES=(); REPO_STATUS=(); }
    load_config
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

@test "CLI mode from a release CLI asks for the directory, saves it, then scans it" {
    [ -z "$SCAN_PATH" ]
    _scan_path_can_ask() { return 0; }
    cmd_repos < <(printf '%s\n' "$HOME/Repositories") > "$TEST_TMPDIR/out"
    grep -q "Where are your git repositories?" "$TEST_TMPDIR/out"
    [ "$(cat "$CALLS")" = "scan $HOME/Repositories" ]
    grep -q "^scan_path = \"$HOME/Repositories\"" "$KRONN_CONFIG_DIR/cli.toml"
}

@test "CLI mode from a release CLI without a terminal fails and scans nothing" {
    _scan_path_can_ask() { return 1; }
    run cmd_repos
    [ "$status" -eq 1 ]
    [[ "$output" == *"No repositories directory configured"* ]]
    [ ! -e "$CALLS" ]
}

@test "status with the backend down skips the scan when no directory is configured" {
    service_is_installed() { return 1; }
    kronn_api_available() { return 1; }
    detect_agents() { :; }
    show_detected_agents() { return 0; }
    run cmd_status
    [ "$status" -eq 0 ]
    [[ "$output" == *"No repositories directory configured"* ]]
    [ ! -e "$CALLS" ]
}

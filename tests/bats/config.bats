#!/usr/bin/env bats
# ─── CLI config/state (lib/config.sh) ────────────────────────────────────────
# Regression tests for the 2026-06-30 key-loss footgun: the CLI must NEVER
# clobber the backend's config.toml (which on Linux/WSL holds encryption_secret
# + auth_token). It writes its own cli.toml instead.

load test_helper

setup() {
    _load_lib "config.sh"
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-config-XXXXXX)"
    export KRONN_CONFIG_DIR="$TEST_TMPDIR/cfg"
    export KRONN_DIR="$TEST_TMPDIR/repo"
    mkdir -p "$KRONN_CONFIG_DIR" "$KRONN_DIR"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

@test "save_config never creates or writes config.toml" {
    export SCAN_PATH="/some/path" SELECTED_AGENT="claude"
    run save_config
    assert_success
    [ ! -f "$KRONN_CONFIG_DIR/config.toml" ]
    [ -f "$KRONN_CONFIG_DIR/cli.toml" ]
}

@test "save_config preserves a pre-existing encryption_secret in config.toml (THE fix)" {
    # Simulate the backend's config.toml holding the key, then run the CLI save.
    # The key MUST survive byte-for-byte — the old cat>config.toml dropped it.
    cat > "$KRONN_CONFIG_DIR/config.toml" <<EOF
encryption_secret = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
[server]
auth_token = "tok-123"
EOF
    local before; before="$(cat "$KRONN_CONFIG_DIR/config.toml")"

    export SCAN_PATH="/x" SELECTED_AGENT="codex"
    run save_config
    assert_success

    local after; after="$(cat "$KRONN_CONFIG_DIR/config.toml")"
    [ "$before" = "$after" ]  # config.toml untouched, byte-for-byte
    run grep -q 'encryption_secret' "$KRONN_CONFIG_DIR/config.toml"
    assert_success
    run grep -q 'auth_token' "$KRONN_CONFIG_DIR/config.toml"
    assert_success
}

@test "save_config writes scan_path + agent to cli.toml" {
    export SCAN_PATH="/repos/here" SELECTED_AGENT="claude"
    run save_config
    assert_success
    run grep -q 'scan_path = "/repos/here"' "$KRONN_CONFIG_DIR/cli.toml"
    assert_success
    run grep -q 'agent = "claude"' "$KRONN_CONFIG_DIR/cli.toml"
    assert_success
}

@test "save_config leaves no temp file behind" {
    export SCAN_PATH="/p" SELECTED_AGENT="x"
    run save_config
    assert_success
    local leftover
    leftover=$(find "$KRONN_CONFIG_DIR" -name 'cli.toml.tmp.*' 2>/dev/null | wc -l)
    [ "$leftover" -eq 0 ]
}

@test "load_config reads scan_path from cli.toml" {
    printf 'scan_path = "/from/cli"\nagent = "x"\n' > "$KRONN_CONFIG_DIR/cli.toml"
    load_config
    [ "$SCAN_PATH" = "/from/cli" ]
}

@test "load_config falls back READ-ONLY to a legacy config.toml scan_path" {
    printf 'scan_path = "/legacy/path"\n' > "$KRONN_CONFIG_DIR/config.toml"
    local before; before="$(cat "$KRONN_CONFIG_DIR/config.toml")"
    load_config
    [ "$SCAN_PATH" = "/legacy/path" ]
    # Must not have rewritten config.toml.
    local after; after="$(cat "$KRONN_CONFIG_DIR/config.toml")"
    [ "$before" = "$after" ]
}

@test "load_config prefers cli.toml over a legacy config.toml value" {
    printf 'scan_path = "/legacy"\n' > "$KRONN_CONFIG_DIR/config.toml"
    printf 'scan_path = "/current"\n' > "$KRONN_CONFIG_DIR/cli.toml"
    load_config
    [ "$SCAN_PATH" = "/current" ]
}

@test "load_config defaults SCAN_PATH to parent of KRONN_DIR when nothing stored" {
    touch "$KRONN_DIR/Makefile"   # a checkout
    load_config
    [ "$SCAN_PATH" = "$(dirname "$KRONN_DIR")" ]
}

# ─── A CLI run from a service release (current/cli) ──────────────────────────

@test "load_config leaves SCAN_PATH empty for a service release CLI, not the service home" {
    # No Makefile: KRONN_DIR is ~/.kronn/service/current/cli, its parent the release.
    load_config
    [ -z "$SCAN_PATH" ]
}

@test "load_config keeps a saved scan_path for a service release CLI" {
    printf 'scan_path = "/from/cli"\n' > "$KRONN_CONFIG_DIR/cli.toml"
    load_config
    [ "$SCAN_PATH" = "/from/cli" ]
}

@test "ensure_scan_path without a terminal says so, fails and saves nothing" {
    _load_lib "ui.sh"
    _load_lib "config.sh"
    SCAN_PATH=""
    _scan_path_can_ask() { return 1; }
    run ensure_scan_path
    [ "$status" -eq 1 ]
    [[ "$output" == *"No repositories directory configured"* ]]
    [ ! -e "$KRONN_CONFIG_DIR/cli.toml" ]
}

@test "ensure_scan_path asks once, expands ~ and saves the directory to cli.toml" {
    _load_lib "ui.sh"
    _load_lib "config.sh"
    export HOME="$TEST_TMPDIR/home"
    mkdir -p "$HOME/Repositories"
    SCAN_PATH=""
    _scan_path_can_ask() { return 0; }
    ensure_scan_path < <(printf '%s\n' "$TEST_TMPDIR/missing" "~/Repositories") > "$TEST_TMPDIR/out"
    [ "$SCAN_PATH" = "$HOME/Repositories" ]
    grep -q "Not a directory: $TEST_TMPDIR/missing" "$TEST_TMPDIR/out"
    grep -q "^scan_path = \"$HOME/Repositories\"" "$KRONN_CONFIG_DIR/cli.toml"
    # Next run: read back, nothing asked.
    SCAN_PATH=""
    _scan_path_can_ask() { echo "asked" >&2; return 1; }
    load_config
    ensure_scan_path
    [ "$SCAN_PATH" = "$HOME/Repositories" ]
}

@test "ensure_scan_path takes HOME on an empty answer" {
    _load_lib "ui.sh"
    _load_lib "config.sh"
    export HOME="$TEST_TMPDIR/home"
    mkdir -p "$HOME"
    SCAN_PATH=""
    _scan_path_can_ask() { return 0; }
    ensure_scan_path < <(printf '\n') > /dev/null
    [ "$SCAN_PATH" = "$HOME" ]
}

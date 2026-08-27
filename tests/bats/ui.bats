#!/usr/bin/env bats
# ─── Tests for lib/ui.sh output functions ────────────────────────────────────

load test_helper

setup() {
    _load_lib "ui.sh"
}

# ─── info ────────────────────────────────────────────────────────────────────

@test "info: outputs the message" {
    run info "Hello world"
    assert_success
    assert_output --partial "Hello world"
}

@test "info: handles multi-word message" {
    run info "This is a longer info message"
    assert_success
    assert_output --partial "This is a longer info message"
}

# ─── success ─────────────────────────────────────────────────────────────────

@test "success: outputs message with checkmark" {
    run success "Operation complete"
    assert_success
    assert_output --partial "Operation complete"
}

@test "success: contains checkmark symbol" {
    run success "done"
    assert_success
    # The raw output contains the checkmark character
    [[ "$output" == *"✓"* ]]
}

# ─── warn ────────────────────────────────────────────────────────────────────

@test "warn: outputs the warning message" {
    run warn "Something may be wrong"
    assert_success
    assert_output --partial "Something may be wrong"
}

@test "warn: contains exclamation symbol" {
    run warn "caution"
    assert_success
    [[ "$output" == *"!"* ]]
}

# ─── fail ────────────────────────────────────────────────────────────────────

@test "fail: outputs the error message" {
    run fail "Something broke"
    assert_success
    assert_output --partial "Something broke"
}

@test "fail: contains cross symbol" {
    run fail "error"
    assert_success
    [[ "$output" == *"✗"* ]]
}

# ─── step ────────────────────────────────────────────────────────────────────

@test "step: outputs the step title" {
    run step "Configuration"
    assert_success
    assert_output --partial "Configuration"
}

@test "step: wraps title with dashes" {
    run step "Setup"
    assert_success
    assert_output --partial "──"
}

# ─── banner ──────────────────────────────────────────────────────────────────

@test "banner: outputs Kronn name" {
    run banner
    assert_success
    assert_output --partial "Kronn"
}

@test "banner: outputs version" {
    run banner
    assert_success
    assert_output --partial "v0.1.0"
}

# ─── Color variables ─────────────────────────────────────────────────────────

@test "color variables: RED is set" {
    [ -n "$RED" ]
}

@test "color variables: GREEN is set" {
    [ -n "$GREEN" ]
}

@test "color variables: YELLOW is set" {
    [ -n "$YELLOW" ]
}

@test "color variables: CYAN is set" {
    [ -n "$CYAN" ]
}

@test "color variables: BOLD is set" {
    [ -n "$BOLD" ]
}

@test "color variables: RESET is set" {
    [ -n "$RESET" ]
}

# ─── Output function return codes ────────────────────────────────────────────

@test "info: returns 0 on success" {
    run info "test"
    assert_success
}

@test "success: returns 0 on success" {
    run success "test"
    assert_success
}

@test "warn: returns 0 on success" {
    run warn "test"
    assert_success
}

@test "fail: returns 0 (output only, not exit)" {
    run fail "test"
    assert_success
}

# ─── Empty message handling ──────────────────────────────────────────────────

@test "info: handles empty message" {
    run info ""
    assert_success
}

@test "success: handles empty message" {
    run success ""
    assert_success
}

@test "warn: handles empty message" {
    run warn ""
    assert_success
}

@test "step: handles empty title" {
    run step ""
    assert_success
}

# ─── Special characters ─────────────────────────────────────────────────────

@test "info: handles special characters" {
    run info 'Message with "quotes" and $vars'
    assert_success
    assert_output --partial "quotes"
}

@test "step: contains separator decoration" {
    run step "Test"
    assert_success
    assert_output --partial "──"
}

# ─── is_macos_host (macOS Docker guard-rail) ──────────────────────────────────

@test "is_macos_host: true for Darwin" {
    run is_macos_host "Darwin"
    assert_success
}

@test "is_macos_host: false for Linux (Docker is the right path there)" {
    run is_macos_host "Linux"
    assert_failure
}

@test "is_macos_host: false for WSL — must NOT regress (the via_wsl host-exec path works)" {
    run is_macos_host "Linux"   # WSL reports `uname -s` = Linux
    assert_failure
}

@test "is_macos_host: false for empty/unknown os" {
    run is_macos_host ""
    assert_failure
}

# ─── dev_missing_tools (kronn start-dev preflight) ────────────────────────────

@test "dev_missing_tools: empty when cargo+node+pnpm+watchexec all present" {
    run dev_missing_tools 1 1 1 1
    assert_success
    assert_output ""
}

@test "dev_missing_tools: lists all four when none present" {
    run dev_missing_tools 0 0 0
    assert_success
    assert_output "cargo node pnpm watchexec"
}

@test "dev_missing_tools: reports only the missing one (pnpm)" {
    run dev_missing_tools 1 1 0 1
    assert_success
    assert_output "pnpm"
}

@test "dev_missing_tools: reports cargo when only Rust is missing" {
    run dev_missing_tools 0 1 1 1
    assert_success
    assert_output "cargo"
}

@test "dev_missing_tools: stable order (cargo before node before pnpm before watchexec)" {
    run dev_missing_tools 0 0 1 1
    assert_success
    assert_output "cargo node"
}

@test "dev_missing_tools: defaults to all-missing when called with no args" {
    run dev_missing_tools
    assert_success
    assert_output "cargo node pnpm watchexec"
}

@test "dev_missing_tools: a non-1 token (e.g. 'yes') counts as missing" {
    run dev_missing_tools yes 1 1 1
    assert_success
    assert_output "cargo"
}

@test "dev_missing_tools: reports watchexec independently" {
    run dev_missing_tools 1 1 1 0
    assert_success
    assert_output "watchexec"
}

@test "dev_backend_watcher_pattern: is specific to Kronn backend watchers" {
    run dev_backend_watcher_pattern
    assert_success
    assert_output "watchexec.*dev-backend-watch-command"
    refute_output "watchexec"
}

@test "wait_for_process_http_ready: succeeds when health answers" {
    curl() { return 0; }
    kill() { return 0; }
    sleep() { :; }

    run wait_for_process_http_ready "http://localhost:3140/api/health" 123 2 0
    assert_success
}

@test "wait_for_process_http_ready: retries while watcher lives" {
    local curl_calls=0
    curl() {
        curl_calls=$((curl_calls + 1))
        [[ "$curl_calls" -ge 2 ]]
    }
    kill() { return 0; }
    sleep() { :; }

    run wait_for_process_http_ready "http://localhost:3140/api/health" 123 3 0
    assert_success
}

@test "wait_for_process_http_ready: distinguishes an exited watcher" {
    curl() { return 1; }
    kill() { return 1; }
    sleep() { :; }

    run wait_for_process_http_ready "http://localhost:3140/api/health" 123 2 0
    assert_failure 2
}

@test "wait_for_process_http_ready: times out while watcher stays alive" {
    curl() { return 1; }
    kill() { return 0; }
    sleep() { :; }

    run wait_for_process_http_ready "http://localhost:3140/api/health" 123 2 0
    assert_failure 1
}

@test "dev_stack_process_state: reports running while both children live" {
    kill() { return 0; }

    run dev_stack_process_state 101 202
    assert_success
    assert_output "running"
}

@test "dev_stack_process_state: reports a dead backend before the frontend" {
    kill() {
        [[ "$2" == "101" ]] && return 1
        return 0
    }

    run dev_stack_process_state 101 202
    assert_failure 2
    assert_output "backend-exited"
}

@test "dev_stack_process_state: reports a dead frontend" {
    kill() {
        [[ "$2" == "202" ]] && return 1
        return 0
    }

    run dev_stack_process_state 101 202
    assert_failure 3
    assert_output "frontend-exited"
}

@test "dev_stack_process_state: rejects missing child identifiers" {
    run dev_stack_process_state "" 202
    assert_failure 2
    assert_output "invalid"
}

@test "dev_backend_exit_is_failure: compile and boot failures are unexpected" {
    run dev_backend_exit_is_failure 1
    assert_success

    run dev_backend_exit_is_failure 101
    assert_success
}

@test "dev_backend_exit_is_failure: clean and watcher signal exits are expected" {
    run dev_backend_exit_is_failure 0
    assert_failure

    run dev_backend_exit_is_failure 130
    assert_failure

    run dev_backend_exit_is_failure 143
    assert_failure
}

@test "dev_backend_exit_is_failure: malformed status fails closed" {
    run dev_backend_exit_is_failure nope
    assert_success
}

@test "dev backend watch command records a cargo failure for the supervisor" {
    local fake_bin="$BATS_TEST_TMPDIR/fake-bin"
    local marker="$BATS_TEST_TMPDIR/backend-failed"
    mkdir -p "$fake_bin"
    printf '#!/usr/bin/env bash\nexit 101\n' >"$fake_bin/cargo"
    chmod +x "$fake_bin/cargo"

    run env \
        PATH="$fake_bin:$PATH" \
        KRONN_DEV_BACKEND_FAILURE_FILE="$marker" \
        "$PROJECT_ROOT/scripts/dev-backend-watch-command.sh"

    assert_failure 101
    run test -f "$marker"
    assert_success
    run sed -n '1p' "$marker"
    assert_output "101"
}

@test "dev backend watch command keeps a healthy supervised backend on compile failure" {
    local fake_bin="$BATS_TEST_TMPDIR/fake-bin"
    local marker="$BATS_TEST_TMPDIR/backend-failed"
    mkdir -p "$fake_bin"
    printf '#!/usr/bin/env bash\nexit 101\n' >"$fake_bin/cargo"
    chmod +x "$fake_bin/cargo"

    run env \
        PATH="$fake_bin:$PATH" \
        KRONN_DEV_BACKEND_FAILURE_FILE="$marker" \
        KRONN_DEV_BACKEND_SUPERVISOR_PID="99999999" \
        "$PROJECT_ROOT/scripts/dev-backend-watch-command.sh"

    assert_failure 101
    assert_output --partial "keeping the current backend online"
    run test -f "$marker"
    assert_failure
}

@test "dev backend watch command ignores a normal watcher restart signal" {
    local fake_bin="$BATS_TEST_TMPDIR/fake-bin"
    local marker="$BATS_TEST_TMPDIR/backend-failed"
    mkdir -p "$fake_bin"
    printf '#!/usr/bin/env bash\nexit 143\n' >"$fake_bin/cargo"
    chmod +x "$fake_bin/cargo"

    run env \
        PATH="$fake_bin:$PATH" \
        KRONN_DEV_BACKEND_FAILURE_FILE="$marker" \
        "$PROJECT_ROOT/scripts/dev-backend-watch-command.sh"

    assert_failure 143
    run test -f "$marker"
    assert_failure
}

@test "dev backend supervisor keeps the old process until a successful-build signal" {
    local fake_bin="$BATS_TEST_TMPDIR/supervisor-bin"
    local fake_backend_dir="$BATS_TEST_TMPDIR/backend"
    local fake_backend="$BATS_TEST_TMPDIR/kronn"
    local starts="$BATS_TEST_TMPDIR/backend-starts"
    mkdir -p "$fake_bin" "$fake_backend_dir"

    printf '#!/usr/bin/env bash\nexit 0\n' >"$fake_bin/cargo"
    printf '#!/usr/bin/env bash\nexit 0\n' >"$fake_bin/curl"
    cat >"$fake_backend" <<'EOF'
#!/usr/bin/env bash
printf 'start\n' >>"$KRONN_TEST_BACKEND_STARTS"
trap 'exit 0' TERM INT
while true; do sleep 1; done
EOF
    cat >"$fake_bin/watchexec" <<'EOF'
#!/usr/bin/env bash
sleep 0.2
kill -USR1 "$KRONN_DEV_BACKEND_SUPERVISOR_PID"
trap 'exit 0' TERM INT
while true; do sleep 1; done
EOF
    chmod +x "$fake_bin/cargo" "$fake_bin/curl" "$fake_bin/watchexec" "$fake_backend"

    run env \
        PATH="$fake_bin:$PATH" \
        KRONN_DEV_BACKEND_DIR="$fake_backend_dir" \
        KRONN_DEV_BACKEND_BINARY="$fake_backend" \
        KRONN_DEV_BACKEND_HEALTH_URL="http://test.invalid/health" \
        KRONN_TEST_BACKEND_STARTS="$starts" \
        bash -c '
            "$1" >/dev/null 2>&1 &
            supervisor=$!
            for _ in $(seq 1 100); do
                [[ "$(wc -l <"$2" 2>/dev/null || echo 0)" -ge 2 ]] && break
                sleep 0.05
            done
            count="$(wc -l <"$2" 2>/dev/null || echo 0)"
            kill -TERM "$supervisor" 2>/dev/null || true
            wait "$supervisor" 2>/dev/null || true
            [[ "$count" -eq 2 ]]
        ' _ "$PROJECT_ROOT/scripts/dev-backend-supervisor.sh" "$starts"

    assert_success
}

@test "dev backend supervisor serves the last successful binary while Cargo is blocked" {
    local fake_bin="$BATS_TEST_TMPDIR/supervisor-warm-bin"
    local fake_backend_dir="$BATS_TEST_TMPDIR/backend-warm"
    local fake_backend="$BATS_TEST_TMPDIR/kronn-warm"
    local starts="$BATS_TEST_TMPDIR/backend-warm-starts"
    local cargo_done="$BATS_TEST_TMPDIR/cargo-done"
    mkdir -p "$fake_bin" "$fake_backend_dir"

    cat >"$fake_bin/cargo" <<'EOF'
#!/usr/bin/env bash
sleep 1
touch "$KRONN_TEST_CARGO_DONE"
EOF
    printf '#!/usr/bin/env bash\nexit 0\n' >"$fake_bin/curl"
    cat >"$fake_backend" <<'EOF'
#!/usr/bin/env bash
printf 'start\n' >>"$KRONN_TEST_BACKEND_STARTS"
trap 'exit 0' TERM INT
while true; do sleep 1; done
EOF
    cat >"$fake_bin/watchexec" <<'EOF'
#!/usr/bin/env bash
trap 'exit 0' TERM INT
while true; do sleep 1; done
EOF
    chmod +x "$fake_bin/cargo" "$fake_bin/curl" "$fake_bin/watchexec" "$fake_backend"

    run env \
        PATH="$fake_bin:$PATH" \
        KRONN_DEV_BACKEND_DIR="$fake_backend_dir" \
        KRONN_DEV_BACKEND_BINARY="$fake_backend" \
        KRONN_DEV_BACKEND_HEALTH_URL="http://test.invalid/health" \
        KRONN_TEST_BACKEND_STARTS="$starts" \
        KRONN_TEST_CARGO_DONE="$cargo_done" \
        bash -c '
            "$1" >/dev/null 2>&1 &
            supervisor=$!
            for _ in $(seq 1 40); do
                [[ -s "$2" ]] && break
                sleep 0.025
            done
            [[ -s "$2" ]]
            [[ ! -e "$3" ]]
            for _ in $(seq 1 80); do
                [[ -e "$3" ]] && break
                sleep 0.025
            done
            kill -TERM "$supervisor" 2>/dev/null || true
            wait "$supervisor" 2>/dev/null || true
        ' _ "$PROJECT_ROOT/scripts/dev-backend-supervisor.sh" "$starts" "$cargo_done"

    assert_success
}

# ─── ask_yn EOF safety (must not loop forever on closed stdin) ─────────────────

@test "ask_yn: returns 1 (no) on EOF instead of looping forever" {
    # </dev/null → not a tty → fallback path → read hits EOF immediately.
    # Before the fix this spun forever; now it must terminate with 'no'.
    run ask_yn "Continue?" </dev/null
    assert_failure
}

@test "ask_yn: 'y' on stdin returns 0 (yes)" {
    run bash -c "source '${PROJECT_ROOT}/lib/ui.sh'; printf 'y\n' | ask_yn 'Continue?'"
    assert_success
}

# ─── hyperlink (OSC 8, cross-platform, TTY-gated) ─────────────────────────────

@test "hyperlink: falls back to the plain URL when stdout is not a TTY" {
    # `run` captures stdout via a pipe → not a TTY → plain URL, no escape bytes.
    run hyperlink "http://localhost:5173"
    assert_success
    assert_output "http://localhost:5173"
}

@test "hyperlink: uses the provided label in the non-TTY fallback" {
    run hyperlink "http://localhost:5173" "open the UI"
    assert_success
    assert_output "open the UI"
}

@test "hyperlink: non-TTY output carries no OSC 8 escape bytes" {
    run hyperlink "http://localhost:5173"
    assert_success
    [[ "$output" != *$'\033]8'* ]]
}

# ─── path_link_action (ensure_in_path: don't re-nag for the symlink) ──────────

@test "path_link_action: already on PATH → ok (no prompt)" {
    run path_link_action 1 0
    assert_success
    assert_output "ok"
}

@test "path_link_action: symlink exists but not on PATH → adopt (no nag)" {
    # The recurring 'Create a symlink?' bug: link is already ours, just adopt it.
    run path_link_action 0 1
    assert_success
    assert_output "adopt"
}

@test "path_link_action: nothing yet → create (offer the symlink)" {
    run path_link_action 0 0
    assert_success
    assert_output "create"
}

@test "path_link_action: on PATH takes precedence over a stray link" {
    run path_link_action 1 1
    assert_success
    assert_output "ok"
}

# ─── pick_opener (cross-platform browser open for start-dev) ──────────────────

@test "pick_opener: prefers wslview (WSL) when present" {
    run pick_opener 1 1 1
    assert_success
    assert_output "wslview"
}

@test "pick_opener: xdg-open on Linux when no wslview" {
    run pick_opener 0 1 1
    assert_success
    assert_output "xdg-open"
}

@test "pick_opener: open on macOS (only opener present)" {
    run pick_opener 0 0 1
    assert_success
    assert_output "open"
}

@test "pick_opener: empty when no opener is available" {
    run pick_opener 0 0 0
    assert_success
    assert_output ""
}

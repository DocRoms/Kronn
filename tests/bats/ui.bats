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

@test "dev_missing_tools: empty when cargo+node+pnpm all present" {
    run dev_missing_tools 1 1 1
    assert_success
    assert_output ""
}

@test "dev_missing_tools: lists all three when none present" {
    run dev_missing_tools 0 0 0
    assert_success
    assert_output "cargo node pnpm"
}

@test "dev_missing_tools: reports only the missing one (pnpm)" {
    run dev_missing_tools 1 1 0
    assert_success
    assert_output "pnpm"
}

@test "dev_missing_tools: reports cargo when only Rust is missing" {
    run dev_missing_tools 0 1 1
    assert_success
    assert_output "cargo"
}

@test "dev_missing_tools: stable order (cargo before node before pnpm)" {
    run dev_missing_tools 0 0 1
    assert_success
    assert_output "cargo node"
}

@test "dev_missing_tools: defaults to all-missing when called with no args" {
    run dev_missing_tools
    assert_success
    assert_output "cargo node pnpm"
}

@test "dev_missing_tools: a non-1 token (e.g. 'yes') counts as missing" {
    run dev_missing_tools yes 1 1
    assert_success
    assert_output "cargo"
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

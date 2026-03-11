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

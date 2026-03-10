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

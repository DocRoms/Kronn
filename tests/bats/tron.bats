#!/usr/bin/env bats
# ─── Tests for lib/tron.sh pure functions ────────────────────────────────────

load test_helper

setup() {
    _load_lib "ui.sh"
    _load_lib "tron.sh"
}

# ─── _tron_format_elapsed ────────────────────────────────────────────────────

@test "_tron_format_elapsed: 0 seconds" {
    run _tron_format_elapsed 0
    assert_success
    assert_output "0m00s"
}

@test "_tron_format_elapsed: 30 seconds" {
    run _tron_format_elapsed 30
    assert_success
    assert_output "0m30s"
}

@test "_tron_format_elapsed: 60 seconds" {
    run _tron_format_elapsed 60
    assert_success
    assert_output "1m00s"
}

@test "_tron_format_elapsed: 90 seconds" {
    run _tron_format_elapsed 90
    assert_success
    assert_output "1m30s"
}

@test "_tron_format_elapsed: 3600 seconds (1 hour)" {
    run _tron_format_elapsed 3600
    assert_success
    assert_output "60m00s"
}

@test "_tron_format_elapsed: 125 seconds" {
    run _tron_format_elapsed 125
    assert_success
    assert_output "2m05s"
}

@test "_tron_format_elapsed: 5 seconds has zero-padded seconds" {
    run _tron_format_elapsed 5
    assert_success
    assert_output "0m05s"
}

# ─── _tron_pad ───────────────────────────────────────────────────────────────

@test "_tron_pad: pads short string to width" {
    _tron_pad "hello" 10
    [ ${#_PAD_RESULT} -eq 10 ]
    [ "$_PAD_RESULT" = "hello     " ]
}

@test "_tron_pad: returns exact string when equal to width" {
    _tron_pad "exact" 5
    [ ${#_PAD_RESULT} -eq 5 ]
    [ "$_PAD_RESULT" = "exact" ]
}

@test "_tron_pad: truncates long string with ellipsis" {
    _tron_pad "this is a very long string" 10
    [ ${#_PAD_RESULT} -eq 10 ]
    # Last 3 chars should be "..."
    [[ "$_PAD_RESULT" == *"..." ]]
}

@test "_tron_pad: single char padded to width 5" {
    _tron_pad "x" 5
    [ ${#_PAD_RESULT} -eq 5 ]
    [ "$_PAD_RESULT" = "x    " ]
}

@test "_tron_pad: empty string padded to width 4" {
    _tron_pad "" 4
    [ ${#_PAD_RESULT} -eq 4 ]
    [ "$_PAD_RESULT" = "    " ]
}

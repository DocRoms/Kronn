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

# ─── tron_init / tron_cleanup ────────────────────────────────────────────────

@test "tron_init: creates temp files" {
    tron_init
    [ -f "$_tron_done_file" ]
    [ -f "$_tron_progress_file" ]
    [ -f "$_tron_step_file" ]
    [ -f "$_tron_prev_step_file" ]
    tron_cleanup
}

@test "tron_init: initializes progress to 0" {
    tron_init
    local val
    val=$(cat "$_tron_progress_file")
    [ "$val" = "0" ]
    tron_cleanup
}

@test "tron_init: sets start time" {
    tron_init
    [ -n "$_tron_start_time" ]
    tron_cleanup
}

@test "tron_cleanup: removes all temp files" {
    tron_init
    local done_f="$_tron_done_file"
    local prog_f="$_tron_progress_file"
    local step_f="$_tron_step_file"
    local prev_f="$_tron_prev_step_file"
    tron_cleanup
    [ ! -f "$done_f" ]
    [ ! -f "$prog_f" ]
    [ ! -f "$step_f" ]
    [ ! -f "$prev_f" ]
}

@test "tron_cleanup: resets state variables" {
    tron_init
    tron_cleanup
    [ -z "$_tron_done_file" ]
    [ -z "$_tron_progress_file" ]
    [ -z "$_tron_step_file" ]
    [ -z "$_tron_start_time" ]
    [ -z "$_tron_target_dir" ]
    [ -z "$_tron_log_file" ]
    [ -z "$_tron_agent_name" ]
}

# ─── tron_progress ───────────────────────────────────────────────────────────

@test "tron_progress: updates progress value" {
    local tmpdir
    tmpdir=$(mktemp -d /tmp/kronn-tron-test-XXXXXX)
    tron_init "$tmpdir"
    tron_progress 42
    local val
    val=$(cat "$_tron_progress_file")
    [ "$val" = "42" ]
    tron_cleanup
    rm -rf "$tmpdir"
}

@test "tron_progress: can update to 100" {
    local tmpdir
    tmpdir=$(mktemp -d /tmp/kronn-tron-test-XXXXXX)
    tron_init "$tmpdir"
    tron_progress 100
    local val
    val=$(cat "$_tron_progress_file")
    [ "$val" = "100" ]
    tron_cleanup
    rm -rf "$tmpdir"
}

# ─── tron_set_step ───────────────────────────────────────────────────────────

@test "tron_set_step: writes step label" {
    tron_init
    tron_set_step "Analyzing code"
    local val
    val=$(cat "$_tron_step_file")
    [ "$val" = "Analyzing code" ]
    tron_cleanup
}

@test "tron_set_step: saves previous step on change" {
    tron_init
    tron_set_step "Step 1"
    tron_set_step "Step 2"
    local prev
    prev=$(cat "$_tron_prev_step_file")
    [ "$prev" = "Step 1" ]
    tron_cleanup
}

# ─── tron_set_log / tron_set_agent ───────────────────────────────────────────

@test "tron_set_log: stores log file path" {
    tron_init
    tron_set_log "/tmp/test.log"
    [ "$_tron_log_file" = "/tmp/test.log" ]
    tron_cleanup
}

@test "tron_set_agent: stores agent name" {
    tron_init
    tron_set_agent "claude"
    [ "$_tron_agent_name" = "claude" ]
    tron_cleanup
}

# ─── tron_signal_done ────────────────────────────────────────────────────────

@test "tron_signal_done: writes done signal" {
    tron_init
    tron_signal_done
    local val
    val=$(cat "$_tron_done_file")
    [ "$val" = "done" ]
    tron_cleanup
}

# ─── tron_init with target dir ───────────────────────────────────────────────

@test "tron_init: creates progress file in target dir" {
    local tmpdir
    tmpdir=$(mktemp -d /tmp/kronn-tron-test-XXXXXX)
    tron_init "$tmpdir"
    [ -f "$tmpdir/.kronn/progress.md" ]
    tron_cleanup
    rm -rf "$tmpdir"
}

@test "tron_cleanup: removes progress file from target dir" {
    local tmpdir
    tmpdir=$(mktemp -d /tmp/kronn-tron-test-XXXXXX)
    tron_init "$tmpdir"
    [ -f "$tmpdir/.kronn/progress.md" ]
    tron_cleanup
    [ ! -f "$tmpdir/.kronn/progress.md" ]
    rm -rf "$tmpdir"
}

@test "tron_progress: updates progress file in target dir" {
    local tmpdir
    tmpdir=$(mktemp -d /tmp/kronn-tron-test-XXXXXX)
    tron_init "$tmpdir"
    tron_progress 50
    run grep "50%" "$tmpdir/.kronn/progress.md"
    assert_success
    tron_cleanup
    rm -rf "$tmpdir"
}

# ─── _tron_write_progress_file ───────────────────────────────────────────────

@test "_tron_write_progress_file: contains checklist format" {
    local tmpdir
    tmpdir=$(mktemp -d /tmp/kronn-tron-test-XXXXXX)
    tron_init "$tmpdir"
    _tron_write_progress_file 30
    run grep '\[x\]' "$tmpdir/.kronn/progress.md"
    assert_success
    run grep '\[ \]' "$tmpdir/.kronn/progress.md"
    assert_success
    tron_cleanup
    rm -rf "$tmpdir"
}

@test "_tron_write_progress_file: marks correct steps as completed at 50%" {
    local tmpdir
    tmpdir=$(mktemp -d /tmp/kronn-tron-test-XXXXXX)
    tron_init "$tmpdir"
    _tron_write_progress_file 50
    # Steps at 10%, 20%, 30%, 40%, 50% should be checked
    local checked
    checked=$(grep -c '\[x\]' "$tmpdir/.kronn/progress.md")
    [ "$checked" -eq 5 ]
    tron_cleanup
    rm -rf "$tmpdir"
}

#!/usr/bin/env bats
# ─── Tests for lib/agents.sh pure functions ──────────────────────────────────

load test_helper

setup() {
    # Source ui.sh first (agents.sh depends on its output functions)
    _load_lib "ui.sh"
    _load_lib "agents.sh"
}

# ─── _parse_version ──────────────────────────────────────────────────────────

@test "_parse_version: extracts semver from 'claude v1.2.3'" {
    run _parse_version "claude v1.2.3"
    assert_success
    assert_output "1.2.3"
}

@test "_parse_version: extracts semver from 'codex 0.1.2-beta'" {
    run _parse_version "codex 0.1.2-beta"
    assert_success
    assert_output "0.1.2"
}

@test "_parse_version: extracts semver from 'vibe version 3.4.5'" {
    run _parse_version "vibe version 3.4.5"
    assert_success
    assert_output "3.4.5"
}

@test "_parse_version: extracts first semver when multiple present" {
    run _parse_version "tool 1.0.0 (built with 2.3.4)"
    assert_success
    assert_output "1.0.0"
}

@test "_parse_version: returns empty for string with no version" {
    run _parse_version "no version here"
    assert_success
    assert_output ""
}

@test "_parse_version: returns empty for empty input" {
    run _parse_version ""
    assert_success
    assert_output ""
}

@test "_parse_version: handles version at start of string" {
    run _parse_version "10.20.30 release"
    assert_success
    assert_output "10.20.30"
}

# ─── _agent_idx ──────────────────────────────────────────────────────────────

@test "_agent_idx: claude is index 0" {
    run _agent_idx "claude"
    assert_success
    assert_output "0"
}

@test "_agent_idx: codex is index 1" {
    run _agent_idx "codex"
    assert_success
    assert_output "1"
}

@test "_agent_idx: vibe is index 2" {
    run _agent_idx "vibe"
    assert_success
    assert_output "2"
}

@test "_agent_idx: gemini is index 3" {
    run _agent_idx "gemini"
    assert_success
    assert_output "3"
}

@test "_agent_idx: unknown agent returns failure" {
    run _agent_idx "unknown_agent"
    assert_failure
    assert_output ""
}

@test "_agent_idx: empty name returns failure" {
    run _agent_idx ""
    assert_failure
    assert_output ""
}

# ─── _count_detected ────────────────────────────────────────────────────────

@test "_count_detected: returns 0 when no agents detected" {
    _AGENT_PATHS=("" "" "" "")
    run _count_detected
    assert_success
    assert_output "0"
}

@test "_count_detected: returns 1 when one agent detected" {
    _AGENT_PATHS=("/usr/bin/claude" "" "" "")
    run _count_detected
    assert_success
    assert_output "1"
}

@test "_count_detected: returns 2 when two agents detected" {
    _AGENT_PATHS=("/usr/bin/claude" "" "/usr/bin/vibe" "")
    run _count_detected
    assert_success
    assert_output "2"
}

@test "_count_detected: returns 4 when all agents detected" {
    _AGENT_PATHS=("/usr/bin/claude" "/usr/bin/codex" "/usr/bin/vibe" "/usr/bin/gemini")
    run _count_detected
    assert_success
    assert_output "4"
}

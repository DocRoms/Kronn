#!/usr/bin/env bats
# ─── Tests for lib/mcps.sh TOML parsing ─────────────────────────────────────

load test_helper

setup() {
    _load_lib "ui.sh"
    _load_lib "mcps.sh"

    # Override KRONN_CONFIG_DIR after sourcing (secret_get reads it at call time)
    KRONN_CONFIG_DIR="$(mktemp -d /tmp/kronn-test-XXXXXX)"
    export KRONN_CONFIG_DIR

    # Create a test secrets.toml fixture
    cat > "$KRONN_CONFIG_DIR/secrets.toml" <<'TOML'
# Test secrets file

[atlassian]
url = "https://mycompany.atlassian.net"
username = "user@example.com"
api_token = "secret-atlassian-token"

[github]
personal_access_token = "ghp_testtoken123"

[aws]
access_key_id = "AKIAIOSFODNN7EXAMPLE"
secret_access_key = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
region = "eu-west-1"

[empty_section]
empty_key = ""
TOML
}

teardown() {
    rm -rf "$KRONN_CONFIG_DIR"
}

# ─── secret_get ──────────────────────────────────────────────────────────────

@test "secret_get: reads atlassian url" {
    run secret_get "atlassian" "url"
    assert_success
    assert_output "https://mycompany.atlassian.net"
}

@test "secret_get: reads atlassian username" {
    run secret_get "atlassian" "username"
    assert_success
    assert_output "user@example.com"
}

@test "secret_get: reads atlassian api_token" {
    run secret_get "atlassian" "api_token"
    assert_success
    assert_output "secret-atlassian-token"
}

@test "secret_get: reads github personal_access_token" {
    run secret_get "github" "personal_access_token"
    assert_success
    assert_output "ghp_testtoken123"
}

@test "secret_get: reads aws region" {
    run secret_get "aws" "region"
    assert_success
    assert_output "eu-west-1"
}

@test "secret_get: reads aws access_key_id" {
    run secret_get "aws" "access_key_id"
    assert_success
    assert_output "AKIAIOSFODNN7EXAMPLE"
}

@test "secret_get: returns empty for missing key in existing section" {
    run secret_get "atlassian" "nonexistent_key"
    assert_success
    assert_output ""
}

@test "secret_get: returns empty for missing section" {
    run secret_get "nonexistent_section" "some_key"
    assert_success
    assert_output ""
}

@test "secret_get: returns empty string for empty value" {
    run secret_get "empty_section" "empty_key"
    assert_success
    assert_output ""
}

@test "secret_get: fails when secrets file does not exist" {
    rm -f "$KRONN_CONFIG_DIR/secrets.toml"
    run secret_get "github" "personal_access_token"
    assert_failure
}

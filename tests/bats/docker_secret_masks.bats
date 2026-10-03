#!/usr/bin/env bats
# KT-962 — the host's secrets are hidden from the backend container.

load test_helper

setup() {
    FAKE_HOME="$BATS_TEST_TMPDIR/home"
    mkdir -p "$FAKE_HOME"
    SCRIPT="$PROJECT_ROOT/scripts/docker-secret-masks.sh"
}

@test "an existing secrets directory or file is covered by an empty read-only mount" {
    mkdir -p "$FAKE_HOME/.config/kronn" "$FAKE_HOME/.aws" "$FAKE_HOME/.ssh"
    touch "$FAKE_HOME/.netrc" "$FAKE_HOME/.git-credentials"
    run "$SCRIPT" "$FAKE_HOME" /e/dir /e/file
    assert_success
    assert_line "      - /e/dir:/host-home/.config/kronn:ro"
    assert_line "      - /e/dir:/host-home/.aws:ro"
    assert_line "      - /e/dir:/host-home/.ssh:ro"
    assert_line "      - /e/file:/host-home/.netrc:ro"
    assert_line "      - /e/file:/host-home/.git-credentials:ro"
}

@test "an absent path is not masked, so no mount point is needed inside the read-only home" {
    run "$SCRIPT" "$FAKE_HOME" /e/dir /e/file
    assert_success
    assert_output ""
}

@test "a file where a directory is expected, or the reverse, is not mounted over" {
    touch "$FAKE_HOME/.aws"
    mkdir -p "$FAKE_HOME/.netrc"
    run "$SCRIPT" "$FAKE_HOME" /e/dir /e/file
    assert_success
    refute_output --partial ".aws"
    refute_output --partial ".netrc"
}

@test "the empty mount sources ship with the repository" {
    [ -d "$PROJECT_ROOT/.docker/empty" ]
    [ -f "$PROJECT_ROOT/.docker/empty-file" ]
    [ ! -s "$PROJECT_ROOT/.docker/empty-file" ]
}

@test "the Azure profile stays visible: the Microsoft 365 plugin reads it through /host-home" {
    mkdir -p "$FAKE_HOME/.azure"
    run "$SCRIPT" "$FAKE_HOME" /e/dir /e/file
    assert_success
    refute_output --partial ".azure"
    grep -q '/host-home/.azure' "$PROJECT_ROOT/backend/scripts/azure-docker-wrapper.sh"
}

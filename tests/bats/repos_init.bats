#!/usr/bin/env bats
# Regression coverage for first-run repository perimeter confirmation (#196).

load test_helper

setup() {
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-repos-init-XXXXXX)"
    export HOME="$TEST_TMPDIR/home"
    mkdir -p "$HOME" "$TEST_TMPDIR/workspace/Kronn" "$TEST_TMPDIR/repos"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

@test "first-run env persists the explicitly confirmed repositories perimeter" {
    local canonical_repos
    canonical_repos="$(cd "$TEST_TMPDIR/repos" && pwd -P)"
    run env \
        HOME="$HOME" \
        KRONN_REPOS_DIR="$TEST_TMPDIR/repos" \
        KRONN_EXTRA_REPOS="$TEST_TMPDIR/extra-one:$TEST_TMPDIR/extra-two" \
        make -s -C "$TEST_TMPDIR/workspace/Kronn" -f "$PROJECT_ROOT/Makefile" .env

    assert_success
    assert_output --partial "Repos dir confirmed: $canonical_repos"
    run grep -F "KRONN_REPOS_DIR=$canonical_repos" "$TEST_TMPDIR/workspace/Kronn/.env"
    assert_success
    run grep -F "KRONN_EXTRA_REPOS=$TEST_TMPDIR/extra-one:$TEST_TMPDIR/extra-two" "$TEST_TMPDIR/workspace/Kronn/.env"
    assert_success
}

@test "checkout directly under HOME refuses the inferred read-only collision with actionable advice" {
    mkdir -p "$HOME/Kronn"

    run env -u KRONN_REPOS_DIR -u KRONN_EXTRA_REPOS \
        HOME="$HOME" \
        make -s -C "$HOME/Kronn" -f "$PROJECT_ROOT/Makefile" .env

    assert_failure
    assert_output --partial "The parent of Kronn becomes the rw repositories directory"
    assert_output --partial "KRONN_EXTRA_REPOS"
}

@test "a primary directory outside HOME gets a safe container target and a WSL performance warning" {
    run env \
        HOME="$HOME" \
        KRONN_REPOS_DIR="/mnt/c/Repositories" \
        make -s -C "$TEST_TMPDIR/workspace/Kronn" -f "$PROJECT_ROOT/Makefile" .env

    assert_success
    assert_output --partial "WSL warning"
    run grep -F "KRONN_REPOS_REL=.kronn-external-repos" "$TEST_TMPDIR/workspace/Kronn/.env"
    assert_success
}

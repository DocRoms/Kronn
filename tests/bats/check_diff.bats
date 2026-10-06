#!/usr/bin/env bats

load test_helper

setup() {
    TEST_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/kronn-check-diff-XXXXXX")"
    cd "$TEST_TMPDIR"
    git init -q -b main .
    git config user.email test@example.invalid
    git config user.name test
    # Pre-existing damage in history must not fail a later, clean change.
    printf 'old   \n' > legacy.txt
    git add legacy.txt
    git commit -q -m base
    BASE="$(git rev-parse HEAD)"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

@test "check-diff: a clean change passes despite older damage" {
    printf 'clean\n' > new.txt
    git add new.txt
    git commit -q -m clean
    run "${PROJECT_ROOT}/scripts/check-diff.sh" "$BASE"
    assert_success
}

@test "check-diff: committed trailing whitespace fails" {
    printf 'bad \n' > new.txt
    git add new.txt
    git commit -q -m bad
    run "${PROJECT_ROOT}/scripts/check-diff.sh" "$BASE"
    assert_failure
    assert_output --partial "new.txt:1: trailing whitespace"
}

@test "check-diff: a committed conflict marker fails" {
    printf '<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n' > notes.md
    git add notes.md
    git commit -q -m conflict
    run "${PROJECT_ROOT}/scripts/check-diff.sh" "$BASE"
    assert_failure
    assert_output --partial "leftover conflict marker"
}

@test "check-diff: unstaged whitespace fails" {
    printf 'dirty \n' >> legacy.txt
    run "${PROJECT_ROOT}/scripts/check-diff.sh" "$BASE"
    assert_failure
}

@test "check-diff: without origin/main, HEAD~1 is the base" {
    printf 'bad \n' > new.txt
    git add new.txt
    git commit -q -m bad
    run "${PROJECT_ROOT}/scripts/check-diff.sh"
    assert_failure
    assert_output --partial "new.txt:1"
}

#!/usr/bin/env bats

load test_helper

setup() {
    TEST_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/kronn-make-nextest-XXXXXX")"
    # A cargo without the nextest subcommand, logging every other call.
    mkdir -p "$TEST_TMPDIR/bin"
    cat > "$TEST_TMPDIR/bin/cargo" <<'SH'
#!/bin/sh
echo "cargo $*" >> "$CARGO_LOG"
[ "$1" = "nextest" ] && exit 101
exit 0
SH
    chmod +x "$TEST_TMPDIR/bin/cargo"
    export CARGO_LOG="$TEST_TMPDIR/cargo.log"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

@test "make test-backend: without nextest it stops, never falls back to cargo test" {
    PATH="$TEST_TMPDIR/bin:$PATH" run make -C "$PROJECT_ROOT" test-backend
    assert_failure
    assert_output --partial "make install-dev-tools"
    run grep -c "cargo test" "$CARGO_LOG"
    assert_output "0"
}

@test "make test-backend-lib: without nextest it stops too" {
    PATH="$TEST_TMPDIR/bin:$PATH" run make -C "$PROJECT_ROOT" test-backend-lib
    assert_failure
    assert_output --partial "make install-dev-tools"
}

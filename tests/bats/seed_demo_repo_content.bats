#!/usr/bin/env bats

load test_helper

assert_file_contains() {
    grep -Fq -- "$2" "$1"
}

setup() {
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-demo-content-XXXXXX)"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

@test "demo content helper creates the public-safe TypeScript project" {
    run "$PROJECT_ROOT/scripts/seed-demo-repo-content.sh" \
        "$TEST_TMPDIR/acme-blog" acme-blog
    assert_success
    [ -s "$TEST_TMPDIR/acme-blog/package.json" ]
    [ -s "$TEST_TMPDIR/acme-blog/src/routes/posts.ts" ]
    [ -s "$TEST_TMPDIR/acme-blog/docs/AGENTS.md" ]
    assert_file_contains "$TEST_TMPDIR/acme-blog/README.md" "fictional"
}

@test "demo content helper creates both halves of the polyglot monorepo" {
    run "$PROJECT_ROOT/scripts/seed-demo-repo-content.sh" \
        "$TEST_TMPDIR/demo-monorepo" demo-monorepo
    assert_success
    [ -s "$TEST_TMPDIR/demo-monorepo/backend/Cargo.toml" ]
    [ -s "$TEST_TMPDIR/demo-monorepo/frontend/package.json" ]
    [ -s "$TEST_TMPDIR/demo-monorepo/docs/architecture.md" ]
}

@test "demo content helper creates the Rust CLI with executable documentation" {
    run "$PROJECT_ROOT/scripts/seed-demo-repo-content.sh" \
        "$TEST_TMPDIR/sample-rust-cli" sample-rust-cli
    assert_success
    [ -s "$TEST_TMPDIR/sample-rust-cli/Cargo.toml" ]
    [ -s "$TEST_TMPDIR/sample-rust-cli/src/count.rs" ]
    assert_file_contains "$TEST_TMPDIR/sample-rust-cli/src/count.rs" \
        "fn counts_words"
    assert_file_contains "$TEST_TMPDIR/sample-rust-cli/docs/usage.md" \
        "cargo test"
}

@test "demo content helper leaves unknown project names untouched" {
    mkdir -p "$TEST_TMPDIR/unknown"
    printf 'sentinel\n' > "$TEST_TMPDIR/unknown/README.md"

    run "$PROJECT_ROOT/scripts/seed-demo-repo-content.sh" \
        "$TEST_TMPDIR/unknown" unknown
    assert_success
    assert_file_contains "$TEST_TMPDIR/unknown/README.md" "sentinel"
}

@test "main screenshot seed isolates MCP sync from the real user home" {
    assert_file_contains "$PROJECT_ROOT/scripts/seed-demo-fixtures.sh" \
        'KRONN_HOST_HOME="$HOST_HOME_DIR"'
    assert_file_contains "$PROJECT_ROOT/scripts/seed-demo-fixtures.sh" \
        'HOST_HOME_DIR="${DATA_DIR}/host-home"'
}

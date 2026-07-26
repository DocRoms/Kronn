#!/usr/bin/env bats

load test_helper

setup() {
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-version-sync-XXXXXX)"
    mkdir -p \
        "$TEST_TMPDIR/backend" \
        "$TEST_TMPDIR/desktop/src-tauri" \
        "$TEST_TMPDIR/frontend" \
        "$TEST_TMPDIR/site"

    printf '1.2.3\n' > "$TEST_TMPDIR/VERSION"
    printf '[package]\nname = "kronn"\nversion = "1.2.3"\n' \
        > "$TEST_TMPDIR/backend/Cargo.toml"
    printf '[[package]]\nname = "kronn"\nversion = "1.2.3"\n' \
        > "$TEST_TMPDIR/backend/Cargo.lock"
    printf '[package]\nname = "kronn-desktop"\nversion = "1.2.3"\n' \
        > "$TEST_TMPDIR/desktop/src-tauri/Cargo.toml"
    printf '[[package]]\nname = "kronn"\nversion = "1.2.3"\n\n[[package]]\nname = "kronn-desktop"\nversion = "1.2.3"\n' \
        > "$TEST_TMPDIR/desktop/src-tauri/Cargo.lock"
    printf '{\n  "version": "1.2.3"\n}\n' > "$TEST_TMPDIR/frontend/package.json"
    printf '{\n  "version": "1.2.3"\n}\n' > "$TEST_TMPDIR/desktop/package.json"
    printf '{\n  "version": "1.2.3"\n}\n' > "$TEST_TMPDIR/desktop/src-tauri/tauri.conf.json"
    printf '## [1.2.3]\n' > "$TEST_TMPDIR/CHANGELOG.md"
    printf '> **Status: 1.2.3 (current release).**\ngit clone --branch 1.2.3 repo\n' \
        > "$TEST_TMPDIR/README.md"
    printf '> **Statut : 1.2.3 (version actuelle).**\ngit clone --branch 1.2.3 repo\n' \
        > "$TEST_TMPDIR/README.fr.md"
    printf 'current v1.2.3\n' > "$TEST_TMPDIR/site/index.html"
    printf 'current v1.2.3\n' > "$TEST_TMPDIR/site/en.html"
    printf 'current v1.2.3\n' > "$TEST_TMPDIR/site/es.html"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

@test "version sync accepts a fully aligned release" {
    run env KRONN_VERSION_ROOT="$TEST_TMPDIR" \
        "$PROJECT_ROOT/scripts/check-version-sync.sh"

    assert_success
    assert_output --partial "Release version 1.2.3 is synchronized everywhere"
}

@test "version sync accepts a dated changelog release heading" {
    printf '## [1.2.3] - 2026-07-27\n' > "$TEST_TMPDIR/CHANGELOG.md"

    run env KRONN_VERSION_ROOT="$TEST_TMPDIR" \
        "$PROJECT_ROOT/scripts/check-version-sync.sh"

    assert_success
    assert_output --partial "Release version 1.2.3 is synchronized everywhere"
}

@test "version sync rejects stale README clone instructions" {
    printf '> **Status: 1.2.3 (current release).**\ngit clone --branch 1.2.2 repo\n' \
        > "$TEST_TMPDIR/README.md"

    run env KRONN_VERSION_ROOT="$TEST_TMPDIR" \
        "$PROJECT_ROOT/scripts/check-version-sync.sh"

    assert_failure
    assert_output --partial "README.md clone instructions do not use 1.2.3"
    assert_output --partial "README.md still contains stale clone version(s): 1.2.2"
}

@test "version sync rejects a stale public-site marker" {
    printf 'current v1.2.2\n' > "$TEST_TMPDIR/site/en.html"

    run env KRONN_VERSION_ROOT="$TEST_TMPDIR" \
        "$PROJECT_ROOT/scripts/check-version-sync.sh"

    assert_failure
    assert_output --partial "site/en.html does not mention v1.2.3"
    assert_output --partial "site/en.html still contains stale current-version marker(s): v1.2.2"
}

@test "version sync rejects a stale manifest" {
    printf '{\n  "version": "1.2.2"\n}\n' > "$TEST_TMPDIR/frontend/package.json"

    run env KRONN_VERSION_ROOT="$TEST_TMPDIR" \
        "$PROJECT_ROOT/scripts/check-version-sync.sh"

    assert_failure
    assert_output --partial "frontend/package.json is '1.2.2' (expected '1.2.3')"
}

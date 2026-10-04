#!/usr/bin/env bats
# ─── entrypoint.sh: ~/.netrc gives each git host only its own token ──────────
#
# The GitHub token must never be offered to gitlab.com. These tests run the
# REAL script with an isolated $HOME and a closed PATH; nothing touches the
# network (ssh-keyscan and curl are stubs).

load test_helper

setup() {
    ENTRYPOINT="${BATS_TEST_DIRNAME}/../../backend/entrypoint.sh"
    FAKE_HOME="${BATS_TEST_TMPDIR}/home"
    STUB_BIN="${BATS_TEST_TMPDIR}/bin"
    mkdir -p "$FAKE_HOME" "$STUB_BIN"

    for real in sh ln mkdir cat git grep chmod awk mv basename rm dirname sed; do
        target="$(command -v "$real" 2>/dev/null || true)"
        case "$target" in /*) ln -sf "$target" "${STUB_BIN}/${real}" ;; esac
    done
    for stub in true ssh-keyscan curl; do
        printf '#!/bin/sh\nexit 0\n' > "${STUB_BIN}/${stub}"
        chmod +x "${STUB_BIN}/${stub}"
    done
}

# Runs the entrypoint with the given extra VAR=value assignments.
_run_entrypoint() {
    run env -i HOME="$FAKE_HOME" PATH="$STUB_BIN" KRONN_HOST_OS=Linux "$@" \
        sh "$ENTRYPOINT" true
}

@test "a GitHub token alone writes no gitlab.com stanza" {
    _run_entrypoint GH_TOKEN=ghp_secret123

    assert_success
    run grep -F "machine github.com" "$FAKE_HOME/.netrc"
    assert_success
    run grep -F "gitlab.com" "$FAKE_HOME/.netrc"
    assert_failure
    run grep -c "ghp_secret123" "$FAKE_HOME/.netrc"
    assert_output "1"
}

@test "a GitHub token alone does not rewrite GitLab SSH remotes to HTTPS" {
    _run_entrypoint GH_TOKEN=ghp_secret123

    assert_success
    run grep -F "https://gitlab.com/" "$FAKE_HOME/.gitconfig"
    assert_failure
    run grep -F "https://github.com/" "$FAKE_HOME/.gitconfig"
    assert_success
}

@test "GITLAB_TOKEN feeds the gitlab.com stanza, never GH_TOKEN" {
    _run_entrypoint GH_TOKEN=ghp_secret123 GITLAB_TOKEN=glpat_other456

    assert_success
    run awk '/^machine gitlab.com$/{s=1;next} /^machine /{s=0} s' "$FAKE_HOME/.netrc"
    assert_output --partial "glpat_other456"
    refute_output --partial "ghp_secret123"
}

@test "a gitlab.com stanza left by an older image is scrubbed" {
    printf 'machine gitlab.com\n  login oauth2\n  password ghp_secret123\nmachine example.org\n  login me\n  password keep\n' \
        > "$FAKE_HOME/.netrc"

    _run_entrypoint GH_TOKEN=ghp_secret123

    assert_success
    run grep -F "gitlab.com" "$FAKE_HOME/.netrc"
    assert_failure
    run grep -F "machine example.org" "$FAKE_HOME/.netrc"
    assert_success
}

@test ".netrc stays owner-only" {
    _run_entrypoint GH_TOKEN=ghp_secret123

    assert_success
    run sh -c "ls -l '$FAKE_HOME/.netrc' | cut -c1-10"
    assert_output "-rw-------"
}

#!/usr/bin/env bats
# ─── entrypoint.sh: conditional Darwin→Linux mirror on a macOS host ──────────
#
# On a macOS host the agent binaries the user installed on their Mac are
# Mach-O and cannot exec() inside the Linux container. `MACOS_HOST_BIN_SKIP`
# (backend/src/agents/mod.rs) makes Kronn ignore them, and entrypoint.sh is
# what puts a runnable Linux copy in their place. When an agent is in the skip
# list but not in entrypoint.sh, nothing replaces it and the agent silently
# degrades to the npx fallback — the exact 2026-04-15 Gemini bug.
#
# These tests run the REAL script. They isolate $HOME (it writes ~/.gitconfig,
# ~/.netrc, ~/.ssh/known_hosts and does `rm -rf ~/.npm/_npx`), pin PATH to a
# built fixture so npm/curl presence is ours to decide, and record what would
# have been installed instead of installing it. Nothing leaves the temp dir.

load test_helper

setup() {
    ENTRYPOINT="${BATS_TEST_DIRNAME}/../../backend/entrypoint.sh"
    AGENTS_RS="${BATS_TEST_DIRNAME}/../../backend/src/agents/mod.rs"

    TEST_TMPDIR="$(mktemp -d /tmp/kronn-entrypoint-XXXXXX)"
    FAKE_HOME="${TEST_TMPDIR}/home"
    FAKE_HOST_BIN="${TEST_TMPDIR}/host-bin"
    STUB_BIN="${TEST_TMPDIR}/bin"
    CALLS="${TEST_TMPDIR}/calls.log"
    mkdir -p "$FAKE_HOME" "$FAKE_HOST_BIN/local" "$STUB_BIN"
    : > "$CALLS"

    # A closed PATH: only what the script genuinely needs, so "npm is missing"
    # is a property of the fixture and not of the developer's machine.
    for real in sh ln mkdir cat git grep chmod awk mv basename rm dirname sed; do
        target="$(command -v "$real" 2>/dev/null || true)"
        # Only an absolute path is linkable: `command -v` answers with the bare
        # name for a shell builtin, and a symlink to that resolves nowhere.
        case "$target" in /*) ln -sf "$target" "${STUB_BIN}/${real}" ;; esac
    done

    # entrypoint.sh ends on `exec "$@"`, which resolves through PATH — a shell
    # builtin will not do. This is the harmless process the container would
    # have become.
    printf '#!/bin/sh\nexit 0\n' > "${STUB_BIN}/true"
    chmod +x "${STUB_BIN}/true"

    # Network- and system-touching commands are always recorded, never run.
    _record_stub ssh-keyscan 0
    _record_stub curl 0
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# Writes an executable that appends its own invocation to $CALLS and exits
# with the requested status. This is how we assert "would have installed"
# without installing.
_record_stub() {
    local name="$1" exit_code="${2:-0}"
    cat > "${STUB_BIN}/${name}" <<STUB
#!/bin/sh
echo "${name} \$*" >> "${CALLS}"
exit ${exit_code}
STUB
    chmod +x "${STUB_BIN}/${name}"
}

# Pretend the user installed <agent> on their Mac: a launcher entry under a
# host-bin mount whose target does not resolve here (a dangling symlink, which
# is exactly what the container sees).
_host_has() {
    ln -sf "/Users/someone/.local/share/$1/bin/$1" "${FAKE_HOST_BIN}/local/$1"
}

# Pretend a real Linux copy already exists inside the container, on PATH and
# outside the host mounts.
_container_has() {
    printf '#!/bin/sh\nexit 0\n' > "${STUB_BIN}/$1"
    chmod +x "${STUB_BIN}/$1"
}

_run_entrypoint() {
    run env -i \
        HOME="$FAKE_HOME" \
        PATH="$STUB_BIN" \
        KRONN_HOST_OS="${KRONN_HOST_OS_OVERRIDE:-macOS}" \
        KRONN_HOST_BIN_ROOT="$FAKE_HOST_BIN" \
        sh "$ENTRYPOINT" true
}

# ═════════════════════════════════════════════════════════════════════════════
# OpenCode parity — the subject of KT-653
# ═════════════════════════════════════════════════════════════════════════════

@test "opencode: a Darwin-only host install is mirrored via npm" {
    _record_stub npm 0
    _host_has opencode

    _run_entrypoint

    assert_success
    run grep -F 'npm install -g opencode-ai' "$CALLS"
    assert_success
}

@test "opencode: an agent the user never installed is never installed for them" {
    _record_stub npm 0
    # No _host_has: nothing under the host mounts.

    _run_entrypoint

    assert_success
    run grep -F 'opencode-ai' "$CALLS"
    assert_failure
}

@test "opencode: an existing in-container Linux copy is not reinstalled" {
    _record_stub npm 0
    _host_has opencode
    _container_has opencode

    _run_entrypoint

    assert_success
    run grep -F 'opencode-ai' "$CALLS"
    assert_failure
}

@test "opencode: a missing npm degrades with a diagnostic, it does not abort the container" {
    _host_has opencode
    # npm deliberately absent from the closed PATH.

    _run_entrypoint

    assert_success
    assert_output --partial "npm missing"
}

@test "opencode: a failing npm install degrades with a diagnostic, it does not abort the container" {
    _record_stub npm 1
    _host_has opencode

    _run_entrypoint

    assert_success
    assert_output --partial "opencode"
    run grep -F 'npm install -g opencode-ai' "$CALLS"
    assert_success
}

@test "opencode: nothing is mirrored when the host is not macOS" {
    _record_stub npm 0
    _host_has opencode
    KRONN_HOST_OS_OVERRIDE="Linux"

    _run_entrypoint

    assert_success
    run grep -F 'opencode-ai' "$CALLS"
    assert_failure
}

# ═════════════════════════════════════════════════════════════════════════════
# The guard that should have caught this one — and will catch the next
# ═════════════════════════════════════════════════════════════════════════════

@test "every agent in MACOS_HOST_BIN_SKIP is handled by entrypoint.sh" {
    # MACOS_HOST_BIN_SKIP says "ignore the host's Darwin binary". entrypoint.sh
    # is the only thing that puts a Linux copy in its place. An agent in the
    # first without the second is silently stuck on the npx fallback.
    local list
    list="$(sed -n '/MACOS_HOST_BIN_SKIP: &\[&str\] = &\[/,/\];/p' "$AGENTS_RS" \
            | grep -o '"[a-z0-9-]*"' | tr -d '"')"

    [ -n "$list" ] || fail "could not read MACOS_HOST_BIN_SKIP from ${AGENTS_RS}"

    local missing=""
    for agent in $list; do
        grep -qE "host_darwin_needs_linux_copy ${agent}( |;|\$)" "$ENTRYPOINT" \
            || missing="${missing} ${agent}"
    done

    [ -z "$missing" ] || fail "in MACOS_HOST_BIN_SKIP but absent from entrypoint.sh:${missing}"
}

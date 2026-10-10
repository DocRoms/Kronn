#!/usr/bin/env bash
# ─── Bats test helper ────────────────────────────────────────────────────────
# Loaded by all .bats test files.

# Project root (two levels up from tests/bats/)
PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Load bats helpers
load "${PROJECT_ROOT}/tests/bats/bats-support/load"
load "${PROJECT_ROOT}/tests/bats/bats-assert/load"

# Fixtures directory
FIXTURES_DIR="${PROJECT_ROOT}/tests/bats/fixtures"

# ─── Source a lib script safely ──────────────────────────────────────────────
# Sources only the file itself without triggering interactive flows.
# Pre-initializes color variables so ui.sh doesn't fail.
_load_lib() {
    local script="$1"

    # Ensure color variables exist (normally set by ui.sh)
    RED=${RED:-$'\033[0;31m'}
    GREEN=${GREEN:-$'\033[0;32m'}
    YELLOW=${YELLOW:-$'\033[0;33m'}
    CYAN=${CYAN:-$'\033[0;36m'}
    BOLD=${BOLD:-$'\033[1m'}
    DIM=${DIM:-$'\033[2m'}
    RESET=${RESET:-$'\033[0m'}
    HIDE_CURSOR=${HIDE_CURSOR:-$'\033[?25l'}
    SHOW_CURSOR=${SHOW_CURSOR:-$'\033[?25h'}
    CLEAR_LINE=${CLEAR_LINE:-$'\033[2K'}
    MOVE_UP=${MOVE_UP:-$'\033[1A'}

    source "${PROJECT_ROOT}/lib/${script}"
    # ui.sh defines a printing-only fail() which returns success. Bats uses
    # its own fail() to propagate assertion errors: never let a loaded app
    # helper replace that assertion boundary. Test the colliding UI function
    # in a separate shell, where it retains its real production behavior.
    source "${PROJECT_ROOT}/tests/bats/bats-support/src/error.bash"
}

# ─── Bounded stop for background fixture processes ───────────────────────────
# A bare `wait` on a fixture that misses its TERM hangs the whole suite until
# CI cancels it. Launch fixtures with `3>&-` (bats waits for every holder of
# its FD 3) and stop them with fixture_stop. Exported for `bash -c` fixtures.
fixture_descendants() { # <pid>
    local child
    for child in $(pgrep -P "$1" 2>/dev/null); do
        printf '%s\n' "$child"
        fixture_descendants "$child"
    done
}

fixture_kill_tree() { # <pid>
    local pid="$1" child
    # Freeze first so the process cannot fork while its children are listed.
    builtin kill -STOP "$pid" 2>/dev/null || true
    for child in $(pgrep -P "$pid" 2>/dev/null); do
        fixture_kill_tree "$child"
    done
    builtin kill -KILL "$pid" 2>/dev/null || true
}

# One TERM only: a second one would abort the fixture's own EXIT-trap cleanup
# and orphan its children. Fails when the process outlives the grace period
# or leaves a child behind; either way nothing keeps running.
fixture_stop() { # <pid> <label> [grace seconds]
    local pid="$1" label="$2" grace="${3:-10}" tick child status=0
    local -a children
    children=($(fixture_descendants "$pid"))
    builtin kill -TERM "$pid" 2>/dev/null || true
    for ((tick = 0; tick < grace * 20; tick++)); do
        builtin kill -0 "$pid" 2>/dev/null || break
        command sleep 0.05
    done
    if builtin kill -0 "$pid" 2>/dev/null; then
        echo "fixture_stop: $label (pid $pid) still running ${grace}s after one TERM; killed its process tree" >&2
        fixture_kill_tree "$pid"
        status=1
        for ((tick = 0; tick < 100; tick++)); do
            builtin kill -0 "$pid" 2>/dev/null || break
            command sleep 0.05
        done
    fi
    if builtin kill -0 "$pid" 2>/dev/null; then
        echo "fixture_stop: $label (pid $pid) survived SIGKILL; not waiting for it" >&2
    else
        wait "$pid" 2>/dev/null || true
    fi
    for child in "${children[@]}"; do
        for ((tick = 0; tick < 40; tick++)); do
            builtin kill -0 "$child" 2>/dev/null || break
            command sleep 0.05
        done
        if builtin kill -0 "$child" 2>/dev/null; then
            echo "fixture_stop: $label left child pid $child running; killed it" >&2
            fixture_kill_tree "$child"
            status=1
        fi
    done
    return "$status"
}
export -f fixture_descendants fixture_kill_tree fixture_stop

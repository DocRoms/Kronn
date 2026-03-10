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
}

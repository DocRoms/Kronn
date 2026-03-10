#!/usr/bin/env bash
# ─── Run all bats tests ─────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BATS_BIN="${SCRIPT_DIR}/bats-core/bin/bats"

if [[ ! -x "$BATS_BIN" ]]; then
    echo "Error: bats-core not found at ${BATS_BIN}"
    echo "Run: git submodule update --init --recursive"
    exit 1
fi

if [[ $# -gt 0 ]]; then
    exec "$BATS_BIN" "$@"
else
    exec "$BATS_BIN" "${SCRIPT_DIR}"/*.bats
fi

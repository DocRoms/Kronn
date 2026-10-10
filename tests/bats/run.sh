#!/usr/bin/env bash
# ─── Run all bats tests ─────────────────────────────────────────────────────
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BATS_BIN="${SCRIPT_DIR}/bats-core/bin/bats"
# A stuck test fails by name ("# timeout after Ns") instead of stalling CI.
# bats 1.13 leaves a failed test's watchdog running: a red run ends this much later.
export BATS_TEST_TIMEOUT="${BATS_TEST_TIMEOUT:-60}"

if [[ ! -x "$BATS_BIN" ]]; then
    echo "Error: bats-core not found at ${BATS_BIN}"
    echo "Run: git submodule update --init --recursive"
    exit 1
fi

if [[ $# -eq 0 ]]; then
    set -- "${SCRIPT_DIR}"/*.bats
fi

# Without a suite deadline, bats owns the process (local default).
if [[ -z "${BATS_SUITE_TIMEOUT:-}" ]]; then
    exec "$BATS_BIN" "$@"
fi

# A process that outlives its test and keeps bats' FD 3 open hangs the run
# after the per-test timeout has fired. The deadline names what is stuck.
descendants() { # <pid>
    local child
    for child in $(pgrep -P "$1" 2>/dev/null); do
        echo "$child"
        descendants "$child"
    done
}

kill_tree() { # <pid>
    local child
    kill -STOP "$1" 2>/dev/null || true
    for child in $(pgrep -P "$1" 2>/dev/null); do
        kill_tree "$child"
    done
    kill -KILL "$1" 2>/dev/null || true
}

"$BATS_BIN" "$@" &
bats_pid=$!
deadline=$((SECONDS + BATS_SUITE_TIMEOUT))
while kill -0 "$bats_pid" 2>/dev/null; do
    if ((SECONDS >= deadline)); then
        echo "bats: suite exceeded BATS_SUITE_TIMEOUT=${BATS_SUITE_TIMEOUT}s" >&2
        stuck="$(descendants "$bats_pid" | paste -sd, -)"
        if [[ -n "$stuck" ]]; then
            ps -o args= -p "$stuck" | awk '$1 ~ /bats-exec-test$/ || $2 ~ /bats-exec-test$/ {
                for (i = 1; i <= NF; i++) if ($i == "--dummy-flag") print "bats: stuck test: " $(i + 2) " in " $(i + 1)
            }' | sort -u >&2
            echo "bats: processes still running:" >&2
            ps -o pid=,ppid=,args= -p "$stuck" >&2 || true
        fi
        kill_tree "$bats_pid"
        wait "$bats_pid" 2>/dev/null || true
        exit 124
    fi
    sleep 1
done
wait "$bats_pid"

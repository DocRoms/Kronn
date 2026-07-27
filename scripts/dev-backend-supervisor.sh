#!/usr/bin/env bash

# Native-development backend supervisor.
#
# The old `watchexec --restart -- cargo run` sequence killed the healthy
# backend BEFORE Cargo rebuilt it. A normal edit therefore left Vite proxying
# to a closed port for the whole compile (or for minutes while Cargo waited on
# a test/clippy build lock). This supervisor keeps the current binary serving,
# lets watchexec build in the background, and swaps processes only after a
# successful build.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
BACKEND_DIR="${KRONN_DEV_BACKEND_DIR:-$PROJECT_ROOT/backend}"
BACKEND_BINARY="${KRONN_DEV_BACKEND_BINARY:-$PROJECT_ROOT/target/debug/kronn}"
HEALTH_URL="${KRONN_DEV_BACKEND_HEALTH_URL:-http://localhost:3140/api/health}"

failure_file="${KRONN_DEV_BACKEND_FAILURE_FILE:-}"
backend_pid=""
watcher_pid=""
reload_requested=0

record_failure() {
    local status="${1:-1}"
    [[ -n "$failure_file" ]] && printf '%s\n' "$status" >"$failure_file"
}

stop_child() {
    local pid="${1:-}"
    [[ -n "$pid" ]] || return 0
    kill "$pid" 2>/dev/null || return 0
    local attempt=0
    while kill -0 "$pid" 2>/dev/null && (( attempt < 100 )); do
        sleep 0.1
        attempt=$((attempt + 1))
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -9 "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    stop_child "$watcher_pid"
    stop_child "$backend_pid"
}

start_backend() {
    (cd "$BACKEND_DIR" && exec "$BACKEND_BINARY") &
    backend_pid=$!
}

backend_ready() {
    local attempt=0
    while (( attempt < 300 )); do
        if curl -fsS --connect-timeout 1 --max-time 1 -o /dev/null \
            "$HEALTH_URL" 2>/dev/null; then
            return 0
        fi
        kill -0 "$backend_pid" 2>/dev/null || return 1
        attempt=$((attempt + 1))
        sleep 0.1
    done
    return 1
}

trap 'reload_requested=1' USR1
trap 'exit 130' INT
trap 'exit 143' TERM
trap cleanup EXIT

[[ -n "$failure_file" ]] && rm -f "$failure_file"

echo "  Building initial backend..."
if ! (cd "$BACKEND_DIR" && cargo build); then
    record_failure 101
    exit 101
fi

start_backend

# A successful watched build signals this supervisor with USR1. `--postpone`
# avoids rebuilding immediately after the explicit initial build above.
KRONN_DEV_BACKEND_SUPERVISOR_PID=$$
export KRONN_DEV_BACKEND_SUPERVISOR_PID
(
    cd "$BACKEND_DIR"
    exec watchexec \
        --postpone \
        --on-busy-update=restart \
        --exts rs,toml,lock \
        --stop-timeout 10s \
        -- ../scripts/dev-backend-watch-command.sh
) &
watcher_pid=$!

while true; do
    if (( reload_requested == 1 )); then
        reload_requested=0
        echo "  Backend build ready — restarting without compile downtime..."
        stop_child "$backend_pid"
        backend_pid=""
        start_backend
        if ! backend_ready; then
            status=1
            if kill -0 "$backend_pid" 2>/dev/null; then
                stop_child "$backend_pid"
            else
                wait "$backend_pid" 2>/dev/null
                status=$?
            fi
            backend_pid=""
            record_failure "$status"
            echo "  Backend failed after the hot-reload swap." >&2
            exit "$status"
        fi
        echo "  Backend hot reload complete."
    fi

    if ! kill -0 "$backend_pid" 2>/dev/null; then
        wait "$backend_pid" 2>/dev/null
        status=$?
        record_failure "${status:-1}"
        exit "${status:-1}"
    fi
    if ! kill -0 "$watcher_pid" 2>/dev/null; then
        wait "$watcher_pid" 2>/dev/null
        status=$?
        record_failure "${status:-1}"
        exit "${status:-1}"
    fi
    sleep 0.2
done

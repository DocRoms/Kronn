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
# shellcheck source=../lib/ui.sh
source "$PROJECT_ROOT/lib/ui.sh"
BACKEND_DIR="${KRONN_DEV_BACKEND_DIR:-$PROJECT_ROOT/backend}"
# Do not inherit a target directory from another checkout.  Cargo's checked-in
# configuration resolves this path at the repository root; exporting it here
# makes that ownership explicit for both the initial and watched builds.
export CARGO_TARGET_DIR="${KRONN_DEV_BACKEND_TARGET_DIR:-$PROJECT_ROOT/target}"
BACKEND_BINARY="${KRONN_DEV_BACKEND_BINARY:-$CARGO_TARGET_DIR/debug/kronn}"
HEALTH_URL="${KRONN_DEV_BACKEND_HEALTH_URL:-http://localhost:3140/api/health}"

failure_file="${KRONN_DEV_BACKEND_FAILURE_FILE:-}"
backend_pid=""
watcher_pid=""
reload_requested=0
bootstrap_started=0
bootstrap_fingerprint=""
serving_fingerprint=""

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
    # A signal can arrive after a fork but before its PID is recorded.
    local child
    for child in $(jobs -pr); do
        stop_child "$child"
    done
}

binary_fingerprint() {
    cksum <"$BACKEND_BINARY" 2>/dev/null || true
}

start_backend() {
    serving_fingerprint="$(binary_fingerprint)"
    (cd "$BACKEND_DIR" && exec "$BACKEND_BINARY") &
    backend_pid=$!
}

backend_ready() {
    # Startup work (including project MCP sync) runs before the HTTP bind on
    # every launch. A reload needs the same readiness budget as the first boot.
    wait_for_process_http_ready \
        "$HEALTH_URL" "$backend_pid" \
        "${KRONN_DEV_READY_ATTEMPTS:-300}" \
        "${KRONN_DEV_READY_INTERVAL:-1}"
}

trap 'reload_requested=1' USR1
trap 'exit 130' INT
trap 'exit 143' TERM
trap cleanup EXIT

[[ -n "$failure_file" ]] && rm -f "$failure_file"

# A repository-wide Cargo target is deliberately shared by the backend,
# desktop shell, tests and clippy.  Another legitimate Cargo command can hold
# that build lock for minutes.  Do not turn that contention into a Kronn
# outage: boot the last successfully-built backend immediately, then let Cargo
# finish in parallel with the already-serving process.  The checksum tells us
# whether the initial build actually replaced the executable and therefore
# needs one swap; an up-to-date build must not interrupt a run for no reason.
if [[ -x "$BACKEND_BINARY" ]]; then
    bootstrap_fingerprint="$(cksum <"$BACKEND_BINARY" 2>/dev/null || true)"
    echo "  Starting the last successful backend while Cargo checks for updates..."
    start_backend
    bootstrap_started=1
fi

echo "  Building initial backend..."
if ! "$SCRIPT_DIR/dev-backend-build-guard.sh"; then
    record_failure 102
    if (( bootstrap_started == 0 )) || ! kill -0 "$backend_pid" 2>/dev/null; then
        exit 102
    fi
    echo "  Initial backend build refused for low disk — keeping the last successful backend online." >&2
elif ! (cd "$BACKEND_DIR" && cargo build); then
    if (( bootstrap_started == 0 )) || ! kill -0 "$backend_pid" 2>/dev/null; then
        record_failure 101
        exit 101
    fi
    echo "  Initial backend build failed — keeping the last successful backend online." >&2
elif (( bootstrap_started == 0 )); then
    start_backend
else
    current_fingerprint="$(cksum <"$BACKEND_BINARY" 2>/dev/null || true)"
    if [[ -z "$bootstrap_fingerprint" || "$current_fingerprint" != "$bootstrap_fingerprint" ]]; then
        echo "  Initial backend build ready — swapping to the updated binary..."
        stop_child "$backend_pid"
        backend_pid=""
        start_backend
    fi
fi

# A successful watched build signals this supervisor with USR1. `--postpone`
# avoids rebuilding immediately after the explicit initial build above.
KRONN_DEV_BACKEND_SUPERVISOR_PID=$$
export KRONN_DEV_BACKEND_SUPERVISOR_PID
if dev_backend_watch_enabled "${KRONN_DEV_BACKEND_WATCH:-1}"; then
    (
        cd "$BACKEND_DIR"
        exec watchexec \
            --postpone \
            --on-busy-update=restart \
            --exts rs,toml,lock \
            --ignore '**/target/**' \
            --stop-timeout 10s \
            -- ../scripts/dev-backend-watch-command.sh
    ) &
    watcher_pid=$!
else
    echo "  Hot reload off — the backend stays up until you stop it."
fi

while true; do
    if (( reload_requested == 1 )) \
        && dev_backend_binary_unchanged "$serving_fingerprint" "$(binary_fingerprint)"; then
        # Any watched file can trigger a build that changes nothing, such as a
        # generated source under a target directory; restarting would only
        # cut the agents the running backend serves.
        reload_requested=0
        echo "  Backend build left the binary unchanged — keeping the running backend."
    fi
    if (( reload_requested == 1 )); then
        reload_requested=0
        echo "  Backend build ready — restarting without compile downtime..."
        stop_child "$backend_pid"
        backend_pid=""
        start_backend
        if backend_ready; then
            echo "  Backend hot reload complete."
        else
            readiness_status=$?
            if [[ "$readiness_status" == "2" ]] || ! kill -0 "$backend_pid" 2>/dev/null; then
                wait "$backend_pid" 2>/dev/null
                status=$?
                echo "  Backend exited before readiness after the hot-reload swap (exit $status)." >&2
                # Even exit 0 is a failed reload if the server never became ready.
                [[ "$status" != "0" ]] || status=1
            else
                status=1
                echo "  Backend readiness timed out after the hot-reload swap ($HEALTH_URL); stopping the still-running backend." >&2
                stop_child "$backend_pid"
            fi
            backend_pid=""
            record_failure "$status"
            exit "$status"
        fi
    fi

    if ! kill -0 "$backend_pid" 2>/dev/null; then
        wait "$backend_pid" 2>/dev/null
        status=$?
        record_failure "${status:-1}"
        exit "${status:-1}"
    fi
    if [[ -n "$watcher_pid" ]] && ! kill -0 "$watcher_pid" 2>/dev/null; then
        wait "$watcher_pid" 2>/dev/null
        status=$?
        record_failure "${status:-1}"
        exit "${status:-1}"
    fi
    sleep 0.2
done

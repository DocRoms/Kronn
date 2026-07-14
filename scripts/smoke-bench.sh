#!/usr/bin/env bash
# Smoke-bench process manager — the guard born from the 0.8.11 smoke P1:
# a `pgrep target/debug/kronn | head -1` killed PRODUCTION (same binary,
# running under cargo watch) instead of the bench instance.
#
# Contract:
#   start <bench_dir>   spawn the backend on the bench dir, CAPTURE the pid
#                       at spawn ($!) into <bench_dir>/bench.pid and mark
#                       the process with KRONN_BENCH=1 in its environment.
#   stop|kill9 <dir>    terminate ONLY the pid from bench.pid, and only
#                       after proving the live process still carries the
#                       bench marker — anything else is refused loudly.
#
# Never identify a process by name/pattern. PID file + env marker, or nothing.
set -euo pipefail

BIN="${KRONN_SMOKE_BIN:-target/debug/kronn}"

usage() { echo "usage: $0 start|stop|kill9 <bench_dir>" >&2; exit 2; }

[ $# -eq 2 ] || usage
CMD="$1"; DIR="$2"
PIDFILE="$DIR/bench.pid"

is_bench_pid() {
    # The pid must be alive AND its environment must carry the marker.
    local pid="$1"
    kill -0 "$pid" 2>/dev/null || return 1
    if [ "$(uname)" = "Darwin" ]; then
        ps -E -p "$pid" 2>/dev/null | grep -q "KRONN_BENCH=1"
    else
        tr '\0' '\n' < "/proc/$pid/environ" 2>/dev/null | grep -qx "KRONN_BENCH=1"
    fi
}

case "$CMD" in
    start)
        [ -d "$DIR" ] || { echo "bench dir not found: $DIR" >&2; exit 1; }
        [ -f "$DIR/kronn.db" ] || { echo "no kronn.db in $DIR — refusing to boot on an empty dir" >&2; exit 1; }
        if [ -f "$PIDFILE" ] && is_bench_pid "$(cat "$PIDFILE")"; then
            echo "bench already running (pid $(cat "$PIDFILE"))" >&2; exit 1
        fi
        KRONN_BENCH=1 KRONN_DATA_DIR="$DIR" RUST_LOG=info \
            "$BIN" > "$DIR/bench.log" 2>&1 &
        echo $! > "$PIDFILE"                      # captured at spawn — the whole point
        echo "bench started: pid $(cat "$PIDFILE"), log $DIR/bench.log"
        ;;
    stop|kill9)
        [ -f "$PIDFILE" ] || { echo "no $PIDFILE — nothing was started here" >&2; exit 1; }
        PID="$(cat "$PIDFILE")"
        # A malformed pidfile is an operator error, never "already gone" —
        # refuse without deleting it, or a live bench would lose its handle.
        if ! [[ "$PID" =~ ^[0-9]+$ ]]; then
            echo "invalid pid in $PIDFILE: '$PID' — refusing to act" >&2
            exit 1
        fi
        if ! kill -0 "$PID" 2>/dev/null; then
            echo "pid $PID already gone"; rm -f "$PIDFILE"; exit 0
        fi
        if ! is_bench_pid "$PID"; then
            echo "REFUSED: pid $PID does not carry KRONN_BENCH=1 — this is NOT a bench process." >&2
            echo "         (pid reuse or wrong dir — never kill an unmarked process.)" >&2
            exit 1
        fi
        if [ "$CMD" = "kill9" ]; then kill -9 "$PID"; else kill "$PID"; fi
        rm -f "$PIDFILE"
        echo "bench $CMD: pid $PID"
        ;;
    *) usage ;;
esac

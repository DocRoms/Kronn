#!/usr/bin/env bash
# RUSTC_WORKSPACE_WRAPPER for the instrumented backend build: the crates of
# this repository compile one at a time, so that two of them never share the
# 16 GB of a hosted runner. Dependencies are not wrapped and keep compiling in
# parallel.
# With KRONN_RUSTC_MEMORY_LOG set, each compilation appends its peak resident
# memory to that file (`<kB> <crate>[ test]`), read by
# backend/scripts/ci/rustc_memory.py. GNU time only: without it nothing is
# recorded, and the workflow step checks that it is there.
set -euo pipefail
lock="${KRONN_RUSTC_LOCK:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/kronn-rustc.lock}"
cmd=("$@")
log="${KRONN_RUSTC_MEMORY_LOG:-}"
if [ -n "$log" ] && /usr/bin/time -f %M true >/dev/null 2>&1; then
  name=""
  kind=""
  previous=""
  for arg in "$@"; do
    [ "$previous" = "--crate-name" ] && name="$arg"
    [ "$arg" = "--test" ] && kind=" test"
    previous="$arg"
  done
  if [ -n "$name" ]; then
    cmd=(/usr/bin/time -a -o "$log" -f "%M ${name}${kind}" "$@")
  fi
fi
if command -v flock >/dev/null 2>&1; then
  exec flock "$lock" "${cmd[@]}"
fi
exec "${cmd[@]}"

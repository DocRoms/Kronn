#!/usr/bin/env bash
# RUSTC_WORKSPACE_WRAPPER for the instrumented backend build: the crates of
# this repository compile one at a time. Instrumented, the library needs about
# 10 GB and its test crate about 20 GB; side by side they exceed the 16 GB of
# a hosted runner and the build swaps past its time limit. Dependencies are
# not wrapped and keep compiling in parallel.
set -euo pipefail
lock="${KRONN_RUSTC_LOCK:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/kronn-rustc.lock}"
if command -v flock >/dev/null 2>&1; then
  exec flock "$lock" "$@"
fi
exec "$@"

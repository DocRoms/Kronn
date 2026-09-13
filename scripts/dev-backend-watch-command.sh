#!/usr/bin/env bash

# Build command run by watchexec for the native development backend.
#
# The supervisor keeps the current backend online while this runs. A successful
# build signals it to perform the short process swap; a compile failure leaves
# the healthy old binary serving and watchexec retries on the next edit.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
# The supervisor exports this for watched builds.  When this command is run on
# its own, keep build artefacts inside this checkout rather than accepting an
# inherited CARGO_TARGET_DIR from another worktree.
export CARGO_TARGET_DIR="${KRONN_DEV_BACKEND_TARGET_DIR:-$PROJECT_ROOT/target}"

# shellcheck source=../lib/ui.sh
source "$PROJECT_ROOT/lib/ui.sh"

failure_file="${KRONN_DEV_BACKEND_FAILURE_FILE:-}"
supervisor_pid="${KRONN_DEV_BACKEND_SUPERVISOR_PID:-}"

set +e
"$SCRIPT_DIR/dev-backend-build-guard.sh"
status=$?
if [[ "$status" == "0" ]]; then
    cargo build
    status=$?
fi
set -e

if [[ "$status" == "0" && "$supervisor_pid" =~ ^[1-9][0-9]*$ ]]; then
    if ! kill -USR1 "$supervisor_pid" 2>/dev/null; then
        echo "  Backend build succeeded, but its dev supervisor is gone." >&2
        exit 1
    fi
elif [[ -n "$supervisor_pid" ]]; then
    if dev_backend_exit_is_failure "$status"; then
        echo "  Backend build failed — keeping the current backend online." >&2
    fi
elif dev_backend_exit_is_failure "$status" && [[ -n "$failure_file" ]]; then
    # Fail closed when invoked outside the supervisor contract.
    printf '%s\n' "$status" >"$failure_file"
fi

exit "$status"

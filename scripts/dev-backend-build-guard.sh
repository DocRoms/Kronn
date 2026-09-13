#!/usr/bin/env bash

# Native development build guard.  This owns no cleanup: it only refuses a
# build before Cargo can allocate more artefacts on the target filesystem.

set -u

target_dir="${CARGO_TARGET_DIR:?CARGO_TARGET_DIR must be set by the dev backend launcher}"
critical_gib="${KRONN_DEV_BACKEND_DISK_CRITICAL_GIB:-5}"

if [[ ! "$critical_gib" =~ ^[0-9]+$ ]]; then
    echo "Invalid KRONN_DEV_BACKEND_DISK_CRITICAL_GIB: $critical_gib" >&2
    exit 2
fi

# The target is deliberately created before the check: Cargo would create it
# anyway, and df must inspect the actual output filesystem rather than cwd.
mkdir -p "$target_dir" || exit 1

available_kib="$(df -Pk "$target_dir" 2>/dev/null | awk 'NR == 2 { print $4 }')"
if [[ ! "$available_kib" =~ ^[0-9]+$ ]]; then
    echo "Refusing native backend build: cannot determine free space for $target_dir." >&2
    exit 1
fi

available_gib=$((available_kib / 1024 / 1024))
if (( available_gib < critical_gib )); then
    echo "Refusing native backend build: only ${available_gib} GiB free at $target_dir (critical below ${critical_gib} GiB). Reclaim owned build artefacts or set KRONN_DEV_BACKEND_DISK_CRITICAL_GIB to the approved server.disk_critical_gib value." >&2
    exit 1
fi

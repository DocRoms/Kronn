#!/bin/sh
set -eu

# Azure CLI always stores its account cache in ~/.azure unless this variable
# is set. Kronn's Docker service mounts the host home read-only at /host-home;
# point the bundled Linux CLI there so a host-side `az login` remains the only
# authentication step and no token is copied into Kronn configuration.
export AZURE_CONFIG_DIR="${KRONN_AZURE_CONFIG_DIR:-/host-home/.azure}"

if [ ! -d "$AZURE_CONFIG_DIR" ]; then
  printf '%s\n' \
    "Azure CLI credentials are unavailable at $AZURE_CONFIG_DIR." \
    "Run 'az login' on the host, then restart or retry Kronn." >&2
  exit 78
fi

exec /usr/bin/az-real "$@"

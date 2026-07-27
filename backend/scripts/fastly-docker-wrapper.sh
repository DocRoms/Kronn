#!/bin/sh
set -eu

# The container runs a Linux Fastly CLI, so its native config lookup would
# target /home/kronn/.config. Point it at the read-only host-home mount instead:
# macOS stores Fastly config under Library/Application Support, while Linux and
# WSL use the XDG config directory.
case "${KRONN_HOST_OS:-Linux}" in
  macOS|Darwin)
    export XDG_CONFIG_HOME="/host-home/Library/Application Support"
    ;;
  *)
    export XDG_CONFIG_HOME="/host-home/.config"
    ;;
esac

exec /usr/local/bin/fastly-real "$@"

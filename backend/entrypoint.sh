#!/bin/sh
set -e

# Restore uv tool symlinks from persistent volume.
# Tools are stored in ~/.local/share/uv/tools/ (volume-backed),
# but the symlinks in ~/.local/bin/ are lost on container rebuild.
UV_TOOLS="${HOME}/.local/share/uv/tools"
UV_BIN="${HOME}/.local/bin"

if [ -d "$UV_TOOLS" ]; then
  mkdir -p "$UV_BIN"
  for tool_dir in "$UV_TOOLS"/*/bin/*; do
    [ -f "$tool_dir" ] || continue
    name=$(basename "$tool_dir")
    ln -sf "$tool_dir" "$UV_BIN/$name" 2>/dev/null
  done
fi

exec "$@"

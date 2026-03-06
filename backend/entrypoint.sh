#!/bin/sh
# Restore uv tool symlinks from persistent volume.
# Tools are stored in /root/.local/share/uv/tools/ (volume-backed),
# but the symlinks in /root/.local/bin/ are lost on container rebuild.
if [ -d /root/.local/share/uv/tools ]; then
  for tool_dir in /root/.local/share/uv/tools/*/bin/*; do
    [ -f "$tool_dir" ] || continue
    name=$(basename "$tool_dir")
    ln -sf "$tool_dir" "/root/.local/bin/$name" 2>/dev/null
  done
fi

exec "$@"

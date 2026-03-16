#!/bin/sh
set -e

# Add GitHub/GitLab SSH host keys to known_hosts (prevents "Host key verification failed")
mkdir -p "${HOME}/.ssh"
if [ ! -f "${HOME}/.ssh/known_hosts" ] || ! grep -q "github.com" "${HOME}/.ssh/known_hosts" 2>/dev/null; then
  ssh-keyscan -t ed25519,rsa github.com gitlab.com >> "${HOME}/.ssh/known_hosts" 2>/dev/null || true
fi

# Forward host SSH agent if available (for git push via SSH)
if [ -S "/run/host-ssh-agent.sock" ]; then
  export SSH_AUTH_SOCK="/run/host-ssh-agent.sock"
fi

# Configure git credential helper for HTTPS push using GH_TOKEN
if [ -n "$GH_TOKEN" ]; then
  git config --global credential.helper '!f() { echo "password=$GH_TOKEN"; }; f' 2>/dev/null || true
  git config --global url."https://x-access-token:${GH_TOKEN}@github.com/".insteadOf "git@github.com:" 2>/dev/null || true
  git config --global url."https://x-access-token:${GH_TOKEN}@github.com/".insteadOf "https://github.com/" 2>/dev/null || true
fi

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

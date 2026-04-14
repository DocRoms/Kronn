#!/bin/sh
set -e

# Symlink host home path → container home so agent configs with hardcoded
# absolute host paths resolve correctly inside the container.
# Example: Vibe's config.toml may have save_dir = "/home/<user>/.vibe/logs/session"
if [ -n "$KRONN_HOST_HOME" ] && [ "$KRONN_HOST_HOME" != "$HOME" ] && [ ! -e "$KRONN_HOST_HOME" ]; then
  ln -sf "$HOME" "$KRONN_HOST_HOME" 2>/dev/null || true
fi

# Global gitignore for Kronn runtime directories (covers all repos + worktrees)
KRONN_GITIGNORE="${HOME}/.kronn-gitignore"
cat > "$KRONN_GITIGNORE" <<'GITIGNORE'
# Kronn runtime (auto-generated — do not edit)
.kronn/
GITIGNORE
git config --global core.excludesFile "$KRONN_GITIGNORE"

# Add GitHub/GitLab SSH host keys to known_hosts (prevents "Host key verification failed")
mkdir -p "${HOME}/.ssh"
if [ ! -f "${HOME}/.ssh/known_hosts" ] || ! grep -q "github.com" "${HOME}/.ssh/known_hosts" 2>/dev/null; then
  ssh-keyscan -t ed25519,rsa github.com gitlab.com >> "${HOME}/.ssh/known_hosts" 2>/dev/null || true
fi

# Forward host SSH agent if available (for git push via SSH)
# macOS Docker Desktop exposes the agent via a special path
if [ -S "/run/host-services/ssh-auth.sock" ]; then
  export SSH_AUTH_SOCK="/run/host-services/ssh-auth.sock"
elif [ -S "/run/host-ssh-agent.sock" ]; then
  export SSH_AUTH_SOCK="/run/host-ssh-agent.sock"
fi

# Configure git credential helper for HTTPS push using GH_TOKEN.
#
# We write the token to ~/.netrc with mode 0600 instead of embedding it in
# `git config credential.helper '!f() { echo "password=$GH_TOKEN"; }; f'`.
# Reasons:
#   1. ~/.gitconfig is world-readable by default (0644) and the inline
#      credential helper exposes the token to anything that can read the file
#      (other agents, MCP servers, log scrapers).
#   2. Inline shell helpers re-evaluate $GH_TOKEN at every git invocation,
#      which means a token containing a quote/newline could break out of the
#      helper string and run arbitrary shell code.
# ~/.netrc with 0600 is the standard mechanism git itself documents and is
# scoped to the current user only.
if [ -n "$GH_TOKEN" ]; then
  NETRC="${HOME}/.netrc"
  # Remove any prior github.com/gitlab.com entries we previously wrote so
  # rotating the token does not leave stale credentials behind.
  if [ -f "$NETRC" ]; then
    awk '
      /^machine (github\.com|gitlab\.com)$/ { skip=1; next }
      /^machine / { skip=0 }
      skip != 1 { print }
    ' "$NETRC" > "${NETRC}.tmp" && mv "${NETRC}.tmp" "$NETRC"
  fi
  {
    echo "machine github.com"
    echo "  login x-access-token"
    echo "  password ${GH_TOKEN}"
    echo "machine gitlab.com"
    echo "  login oauth2"
    echo "  password ${GH_TOKEN}"
  } >> "$NETRC"
  chmod 600 "$NETRC"

  # Make HTTPS the canonical remote so SSH-style URLs are rewritten on the fly.
  # The token is sourced from ~/.netrc, never inlined in the URL, so it does
  # not end up in `git config --list` or process listings.
  git config --global url."https://github.com/".insteadOf "git@github.com:" 2>/dev/null || true
  git config --global url."https://gitlab.com/".insteadOf "git@gitlab.com:" 2>/dev/null || true
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

# On macOS hosts, never rely on host-mounted kiro-cli (Darwin binary).
# Ensure a Linux kiro-cli is present in the container.
PATH="${HOME}/.local/bin:${PATH}"
if [ "${KRONN_HOST_OS:-}" = "macOS" ]; then
  if ! command -v kiro-cli >/dev/null 2>&1; then
    echo "[entrypoint] macOS host detected: installing Linux kiro-cli..."
    if command -v unzip >/dev/null 2>&1; then
      if ! curl -fsSL https://cli.kiro.dev/install | bash; then
        echo "[entrypoint] warning: kiro-cli install failed (Kiro unavailable until fixed)."
      fi
    else
      echo "[entrypoint] warning: unzip missing, cannot install kiro-cli."
    fi
  fi

  # Same for Claude Code — the host's macOS `claude` binary can't run in
  # this Linux container. Install a Linux version via npm.
  if ! command -v claude >/dev/null 2>&1; then
    echo "[entrypoint] macOS host detected: installing Linux claude via npm..."
    if command -v npm >/dev/null 2>&1; then
      if ! npm install -g @anthropic-ai/claude-code 2>/dev/null; then
        echo "[entrypoint] warning: claude-code install failed (Claude Code unavailable until fixed)."
      fi
    else
      echo "[entrypoint] warning: npm missing, cannot install claude-code."
    fi
  fi

  # Same for Codex
  if ! command -v codex >/dev/null 2>&1; then
    echo "[entrypoint] macOS host detected: installing Linux codex via npm..."
    if command -v npm >/dev/null 2>&1; then
      npm install -g @openai/codex 2>/dev/null || true
    fi
  fi
fi

exec "$@"

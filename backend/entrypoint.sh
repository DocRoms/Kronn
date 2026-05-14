#!/bin/sh
set -e

# Symlink host home path → container home so agent configs with hardcoded
# absolute host paths resolve correctly inside the container.
# Example: Vibe's config.toml may have save_dir = "/home/<user>/.vibe/logs/session"
if [ -n "$KRONN_HOST_HOME" ] && [ "$KRONN_HOST_HOME" != "$HOME" ] && [ ! -e "$KRONN_HOST_HOME" ]; then
  ln -sf "$HOME" "$KRONN_HOST_HOME" 2>/dev/null || true
fi

# Bridge agent installs that live under ~/.local/share/<tool> on the host.
#
# The launcher symlinks at ~/.local/bin/<tool> (mounted at /host-bin/local/<tool>
# in the container) store the LITERAL host path of their target — typically
# /home/<host_user>/.local/share/<tool>/<version>/binary. Inside the container
# `~/.local/share` is mounted at /host-home/.local/share, NOT under $HOME, so
# those symlinks resolve into the void → spawning the direct binary fails and
# Kronn silently falls back to `npx`.
#
# Why we care: the npx fallback wraps the agent in an additional Node.js
# process. On long Claude Code sessions (heavy implementation steps, > 20 min)
# the npx-wrapped path crashed with `exit 1` and no stderr — a UX-breaking
# regression for the workflow runner. Resolving the direct binary fixes the
# whole class of issues since the standalone Bun-bundled binary (Claude),
# Codex CLI, etc. handle their own lifecycle without an extra runtime.
#
# The bridge: drop a per-tool symlink under ~/.local/share so the host paths
# resolve. Idempotent — only creates the symlink when the target dir exists
# under /host-home and the link doesn't already exist locally.
if [ -d "/host-home/.local/share" ]; then
  mkdir -p "${HOME}/.local/share"
  for tool in claude vibe codex copilot gemini kiro junie; do
    src="/host-home/.local/share/${tool}"
    dst="${HOME}/.local/share/${tool}"
    if [ -d "$src" ] && [ ! -e "$dst" ]; then
      ln -sf "$src" "$dst" 2>/dev/null || true
    fi
  done
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

  # Same for Gemini CLI — bug reported 2026-04-15 where macOS users never
  # saw Gemini detected because the host Darwin binary was silently
  # skipped but nothing replaced it inside the container.
  if ! command -v gemini >/dev/null 2>&1; then
    echo "[entrypoint] macOS host detected: installing Linux gemini via npm..."
    if command -v npm >/dev/null 2>&1; then
      npm install -g @google/gemini-cli 2>/dev/null || true
    fi
  fi

  # Same for GitHub Copilot CLI
  if ! command -v copilot >/dev/null 2>&1; then
    echo "[entrypoint] macOS host detected: installing Linux copilot via npm..."
    if command -v npm >/dev/null 2>&1; then
      npm install -g @github/copilot 2>/dev/null || true
    fi
  fi
fi

# 0.8.3 (#313) — npm `_npx` cache is corrupted by parallel `npx -y …` calls
# that race on `node_modules/<pkg>` rename. Concrete symptom observed
# during the DOCROMS_WEB audit Step 8 (MCP introspection runs 4 npx
# servers — context7, sequential-thinking, memory, sometimes more — in
# the same window): `npm ERR! ENOTEMPTY: rename .../ajv → .ajv-<hash>`
# leaves the install half-baked, and EVERY subsequent invocation of
# any of these MCPs fails to start. The agent then describes the MCP
# as "tools not exposed" and inserts a `TODO: ask user` marker — even
# though the server is configured correctly.
#
# Fix: nuke the per-user `_npx` cache on container start. npx will
# re-download the package next time it's launched, which is at most
# a 5-10 s cold-start per package (already documented + tolerated by
# the audit prompt). A clean state on every container start trades
# one cold-start for permanent correctness.
#
# We do this AFTER all the symlink/mount bridging above so a stale
# cache from a previous container life doesn't poison the run.
if [ -d "${HOME}/.npm/_npx" ]; then
  rm -rf "${HOME}/.npm/_npx" 2>/dev/null || true
fi

exec "$@"

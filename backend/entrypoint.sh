#!/bin/sh
set -e

# ── Run as the host user, never as root (KT-1468) ────────────────────────────
# A published image cannot know the UID/GID that owns the user's bind mounts
# (~/.claude, ~/.ssh, the repositories, …), so the `kronn` user is aligned with
# KRONN_HOST_UID / KRONN_HOST_GID when the container starts. The container
# starts as root only for this block, then re-executes THIS script as that
# user: everything below, the backend and every agent it spawns, runs
# unprivileged. When no remap is needed (the ids already match: an image built
# locally for this user, or a host user 1000:1000) only the ownership checks
# remain, and they change nothing.
#
# KRONN_PASSWD_FILE / KRONN_GROUP_FILE / KRONN_MOUNTINFO exist so the logic can
# be exercised outside a container; production never sets them.
KRONN_PASSWD_FILE="${KRONN_PASSWD_FILE:-/etc/passwd}"
KRONN_GROUP_FILE="${KRONN_GROUP_FILE:-/etc/group}"
KRONN_MOUNTINFO="${KRONN_MOUNTINFO:-/proc/self/mountinfo}"

# Leading zeros carry no value ("0000000007" is 7, "000" is root, and a
# leading zero must never reach a tool that reads it as octal).
kronn_strip_zeros() { # <digits> -> $kronn_stripped
  kronn_stripped="$1"
  while [ "${#kronn_stripped}" -gt 1 ] && [ "${kronn_stripped#0}" != "$kronn_stripped" ]; do
    kronn_stripped="${kronn_stripped#0}"
  done
}

# A usable host id: numeric, not root (agents must never run as uid/gid 0), and
# a 32-bit id other than the 4294967295 "nobody" sentinel: 1..4294967294. Ids
# above nine digits are real (LDAP/AD-mapped users, 1000000000). The bound is
# compared in two five-digit halves, so `sh` arithmetic never overflows.
kronn_valid_id() {
  case "$1" in ''|*[!0-9]*) return 1 ;; esac
  kronn_strip_zeros "$1"
  id="$kronn_stripped"
  [ "$id" != "0" ] || return 1
  [ "${#id}" -le 10 ] || return 1
  [ "${#id}" -eq 10 ] || return 0
  id_high="${id%?????}"
  id_low="${id#"$id_high"}"
  kronn_strip_zeros "$id_low"
  id_low="$kronn_stripped"
  [ "$id_high" -lt 42949 ] || { [ "$id_high" -eq 42949 ] && [ "$id_low" -le 67294 ]; }
}

# `uid:gid` owning a path (GNU stat in the image, BSD stat on a developer Mac).
kronn_owner_of() {
  stat -c '%u:%g' "$1" 2>/dev/null || stat -f '%u:%g' "$1" 2>/dev/null
}

kronn_passwd_field() { # <field number>
  awk -F: -v n="$1" '$1 == "kronn" { print $n; exit }' "$KRONN_PASSWD_FILE"
}

# Rewrite a file in place through a temp copy (keeps the inode and mode).
kronn_rewrite() { # <file> <awk args…>
  target="$1"
  shift
  awk "$@" "$target" > "$target.kronn.tmp"
  cat "$target.kronn.tmp" > "$target"
  rm -f "$target.kronn.tmp"
}

# What the image itself created for the old ids (the home tree, the docs venv,
# ccusage). Only that filesystem: every mount below it (the user's bind mounts,
# the named volumes) is pruned, so nothing of the host is touched here. A
# `find -user OLD` rather than `chown -R`: it leaves alone any file the old ids
# do not own. A function: `set --` below must not replace the entrypoint's args.
kronn_chown_image_trees() {
  for tree in "$kronn_home" /usr/local/lib/node_modules/ccusage; do
    [ -d "$tree" ] || continue
    set -- "$tree" -xdev
    if [ -r "$KRONN_MOUNTINFO" ]; then
      for mount_point in $(awk '{ print $5 }' "$KRONN_MOUNTINFO"); do
        case "$mount_point" in
          *\\*) ;;  # an escaped (space-bearing) path: cannot be pruned by name
          "$tree"/*) set -- "$@" -path "$mount_point" -prune -o ;;
        esac
      done
    fi
    find "$@" \( -user "$cur_uid" -o -group "$cur_gid" \) \
      -exec chown -h "$want_uid:$want_gid" {} + 2>/dev/null || true
  done
}

kronn_drop_to_host_user() {
  cur_uid="$(kronn_passwd_field 3)"
  cur_gid="$(kronn_passwd_field 4)"
  kronn_home="$(kronn_passwd_field 6)"
  if [ -z "$cur_uid" ] || [ -z "$kronn_home" ]; then
    echo "[entrypoint] fatal: no 'kronn' user in this image; refusing to run as root." >&2
    exit 1
  fi

  want_uid="${KRONN_HOST_UID:-}"
  want_gid="${KRONN_HOST_GID:-}"
  if ! kronn_valid_id "$want_uid"; then
    [ -z "$want_uid" ] || echo "[entrypoint] warning: KRONN_HOST_UID='$want_uid' is not a non-root uid; keeping $cur_uid." >&2
    want_uid="$cur_uid"
  else
    kronn_strip_zeros "$want_uid"
    want_uid="$kronn_stripped"
  fi
  if ! kronn_valid_id "$want_gid"; then
    [ -z "$want_gid" ] || echo "[entrypoint] warning: KRONN_HOST_GID='$want_gid' is not a non-root gid; keeping $cur_gid." >&2
    want_gid="$cur_gid"
  else
    kronn_strip_zeros "$want_gid"
    want_gid="$kronn_stripped"
  fi
  # Whatever the image's own user says: the drop never lands on root.
  if [ "$want_uid" = "0" ] || [ "$want_gid" = "0" ]; then
    echo "[entrypoint] fatal: the kronn user resolves to uid $want_uid / gid $want_gid; refusing to run as root." >&2
    exit 1
  fi

  if [ "$want_gid" != "$cur_gid" ]; then
    # The group named `kronn` must own the gid. On macOS the host gid is 20,
    # which is `dialout` in Debian: take that entry over (as the build-time
    # remap always did) so `chown kronn:kronn` keeps meaning the right gid.
    kronn_rewrite "$KRONN_GROUP_FILE" -F: -v OFS=: -v gid="$want_gid" '
      $1 == "kronn" && $3 != gid { next }
      $3 == gid { $1 = "kronn"; seen = 1 }
      { print }
      END { if (!seen) print "kronn:x:" gid ":" }
    '
  fi
  if [ "$want_uid" != "$cur_uid" ] || [ "$want_gid" != "$cur_gid" ]; then
    # Same for the uid: a host user with a low or system-range uid (7 is `lp`)
    # lands on an image account. Two passwd entries at one uid make getpwuid
    # answer with the first, so ssh, git, `whoami` and the language runtimes
    # would resolve the old account's name and home instead of kronn's. kronn
    # takes the uid over and the old entry goes (its group is left alone).
    taken_by="$(awk -F: -v uid="$want_uid" '$1 != "kronn" && $3 == uid { printf "%s ", $1 }' "$KRONN_PASSWD_FILE")"
    [ -z "$taken_by" ] || echo "[entrypoint] note: uid $want_uid belonged to image user(s) ${taken_by% }; the kronn user takes it over." >&2
    kronn_rewrite "$KRONN_PASSWD_FILE" -F: -v OFS=: -v uid="$want_uid" -v gid="$want_gid" '
      $1 != "kronn" && $3 == uid { next }
      $1 == "kronn" { $3 = uid; $4 = gid }
      { print }
    '

    kronn_chown_image_trees
  fi

  # Named volumes keep the owner their mount point had when they were created
  # (the image's ids): fix a volume whose top directory is not ours. Only when
  # needed: a volume already owned by the right user is never walked.
  for volume in "${KRONN_DATA_DIR:-/data}" \
      "$kronn_home/.npm" "$kronn_home/.cache/uv" "$kronn_home/.local/share/uv" \
      "$kronn_home/.claude/projects" "$kronn_home/.claude/sessions"; do
    [ -d "$volume" ] || continue
    if [ "$(kronn_owner_of "$volume")" != "$want_uid:$want_gid" ]; then
      chown -R "$want_uid:$want_gid" "$volume" 2>/dev/null \
        || echo "[entrypoint] warning: could not hand $volume to $want_uid:$want_gid." >&2
    fi
  done

  # Keep the supplementary groups compose added (`group_add`, e.g. the Docker
  # socket's, opt-in) but never root's: the agents get gid 0 from nowhere.
  groups="$want_gid"
  for group_id in $(id -G); do
    case ",$groups," in *",$group_id,"*) continue ;; esac
    [ "$group_id" = "0" ] || groups="$groups,$group_id"
  done

  command -v setpriv >/dev/null 2>&1 || {
    echo "[entrypoint] fatal: setpriv (util-linux) is missing; refusing to run as root." >&2
    exit 1
  }
  HOME="$kronn_home"
  export HOME
  exec setpriv --reuid="$want_uid" --regid="$want_gid" --groups="$groups" "$0" "$@"
}

if [ "$(id -u)" = "0" ]; then
  kronn_drop_to_host_user "$@"
fi

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
# Each host gets only its own token: the GitHub token is never offered to
# gitlab.com (GITLAB_TOKEN alone feeds that stanza).
NETRC="${HOME}/.netrc"
if [ -n "$GH_TOKEN" ] || [ -n "$GITLAB_TOKEN" ] || [ -f "$NETRC" ]; then
  # Remove any prior github.com/gitlab.com entries we previously wrote so
  # rotating the token does not leave stale credentials behind (including a
  # gitlab.com entry that older images filled with the GitHub token).
  if [ -f "$NETRC" ]; then
    awk '
      /^machine (github\.com|gitlab\.com)$/ { skip=1; next }
      /^machine / { skip=0 }
      skip != 1 { print }
    ' "$NETRC" > "${NETRC}.tmp" && mv "${NETRC}.tmp" "$NETRC"
  fi
  if [ -n "$GH_TOKEN" ]; then
    {
      echo "machine github.com"
      echo "  login x-access-token"
      echo "  password ${GH_TOKEN}"
    } >> "$NETRC"
  fi
  if [ -n "$GITLAB_TOKEN" ]; then
    {
      echo "machine gitlab.com"
      echo "  login oauth2"
      echo "  password ${GITLAB_TOKEN}"
    } >> "$NETRC"
  fi
  [ -f "$NETRC" ] && chmod 600 "$NETRC"

  # Make HTTPS the canonical remote so SSH-style URLs are rewritten on the fly,
  # only for a host we hold a token for (an SSH-only GitLab user keeps SSH).
  # The token is sourced from ~/.netrc, never inlined in the URL, so it does
  # not end up in `git config --list` or process listings.
  if [ -n "$GH_TOKEN" ]; then
    git config --global url."https://github.com/".insteadOf "git@github.com:" 2>/dev/null || true
  fi
  if [ -n "$GITLAB_TOKEN" ]; then
    git config --global url."https://gitlab.com/".insteadOf "git@gitlab.com:" 2>/dev/null || true
  else
    git config --global --unset-all url."https://gitlab.com/".insteadOf 2>/dev/null || true
  fi
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

# On macOS hosts, an agent the user installed on their Mac is a Darwin binary
# mounted read-only at /host-bin/local — it can't exec() in this Linux
# container. For those (and ONLY those) we install a runnable Linux copy.
PATH="${HOME}/.local/bin:${PATH}"

# True (exit 0) when the user has `$1` installed on their Mac (so we must
# mirror a runnable Linux copy into the container) AND no in-container Linux
# copy exists yet.
#
# Detection is by PRESENCE of the launcher entry under a host-bin mount
# (/host-bin/*), NOT by `command -v`: the host launchers are symlinks whose
# target is a Darwin binary under a host path that doesn't resolve in this
# Linux container, so `command -v claude` returns *nothing* (the symlink looks
# dangling) — a `command -v` test would wrongly conclude "absent" and skip the
# install, leaving Claude stuck on the fragile npx fallback. `[ -L ]` matches a
# symlink even when its target doesn't resolve here. Exec-free on purpose.
#
# Deliberately FALSE when the binary is absent from every host mount: Kronn
# must not install agents the user never had — the UI offers an explicit
# "Install" button for those. Also FALSE once a real in-container Linux copy is
# on PATH outside the host mounts (idempotent across reboots).
#
# The host mounts live under /host-bin in the container. The root is read from
# KRONN_HOST_BIN_ROOT so the mirror can be exercised by tests outside a
# container, where /host-bin cannot be created without root. Production never
# sets it, so the default is the only path that ever runs there.
HOST_BIN_ROOT="${KRONN_HOST_BIN_ROOT:-/host-bin}"

host_darwin_needs_linux_copy() {
  # A real in-container Linux copy already on PATH (outside the host mounts)?
  case "$(command -v "$1" 2>/dev/null || true)" in
    ""|"$HOST_BIN_ROOT"/* ) : ;;   # absent here, or only the host shadow → keep checking
    * )                     return 1 ;;  # genuine in-container copy → nothing to do
  esac
  # Did the user install it on their Mac? = a (possibly dangling) launcher
  # entry under any host-bin mount.
  for _d in "$HOST_BIN_ROOT"/*; do
    if [ -e "$_d/$1" ] || [ -L "$_d/$1" ]; then
      return 0
    fi
  done
  return 1
}

if [ "${KRONN_HOST_OS:-}" = "macOS" ]; then
  if host_darwin_needs_linux_copy kiro-cli; then
    echo "[entrypoint] macOS host detected: mirroring host Kiro into a Linux kiro-cli..."
    if command -v unzip >/dev/null 2>&1; then
      if ! curl -fsSL https://cli.kiro.dev/install | bash; then
        echo "[entrypoint] warning: kiro-cli install failed (Kiro unavailable until fixed)."
      fi
    else
      echo "[entrypoint] warning: unzip missing, cannot install kiro-cli."
    fi
  fi

  # Claude Code — the host's macOS `claude` binary can't run in this Linux
  # container. A native Linux copy is strongly preferred over the npx fallback,
  # which has crashed on long (> 20 min) Claude sessions (see the bridge note
  # above). Install via npm whenever only the Darwin shadow is reachable.
  if host_darwin_needs_linux_copy claude; then
    echo "[entrypoint] macOS host detected: installing Linux claude via npm..."
    if command -v npm >/dev/null 2>&1; then
      if ! npm install -g @anthropic-ai/claude-code 2>/dev/null; then
        echo "[entrypoint] warning: claude-code install failed (Claude Code falls back to npx runtime)."
      fi
    else
      echo "[entrypoint] warning: npm missing, cannot install claude-code."
    fi
  fi

  # Same for Codex
  if host_darwin_needs_linux_copy codex; then
    echo "[entrypoint] macOS host detected: installing Linux codex via npm..."
    if command -v npm >/dev/null 2>&1; then
      npm install -g @openai/codex 2>/dev/null || true
    fi
  fi

  # Same for OpenCode — it runs as a native ACP runtime, and without a Linux
  # copy the skipped Darwin binary leaves it on the npx fallback.
  if host_darwin_needs_linux_copy opencode; then
    echo "[entrypoint] macOS host detected: installing Linux opencode via npm..."
    if command -v npm >/dev/null 2>&1; then
      if ! npm install -g opencode-ai 2>/dev/null; then
        echo "[entrypoint] warning: opencode install failed (OpenCode falls back to npx runtime)."
      fi
    else
      echo "[entrypoint] warning: npm missing, cannot install opencode."
    fi
  fi

  # Same for Gemini CLI — bug reported 2026-04-15 where macOS users never
  # saw Gemini detected because the host Darwin binary was silently
  # skipped but nothing replaced it inside the container.
  if host_darwin_needs_linux_copy gemini; then
    echo "[entrypoint] macOS host detected: installing Linux gemini via npm..."
    if command -v npm >/dev/null 2>&1; then
      npm install -g @google/gemini-cli 2>/dev/null || true
    fi
  fi

  # Same for GitHub Copilot CLI
  if host_darwin_needs_linux_copy copilot; then
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

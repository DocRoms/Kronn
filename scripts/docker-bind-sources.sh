#!/usr/bin/env bash
# Create, as the calling user, every ${HOME} bind-mount source that
# docker-compose.yml declares and that does not exist yet.
#
# Docker creates a missing bind source itself, owned by root, inside the
# user's home; a file source then becomes a directory. Running this before
# `docker compose up` keeps those paths the user's own.
#
# Usage: docker-bind-sources.sh [compose-file] [home]
set -euo pipefail

compose="${1:-docker-compose.yml}"
home="${2:-$HOME}"

# Mounted as files: everything else is a directory. `.claude.json` is read as
# JSON by Claude Code, so it starts as an empty object, never an empty file.
file_sources=(.claude.json)

is_file_source() {
    local rel="$1" known
    for known in "${file_sources[@]}"; do
        [[ "$rel" == "$known" ]] && return 0
    done
    return 1
}

# `- ${HOME}/x:/target[:mode]` → `x`; the bare `${HOME}` mount is the home itself.
sed -n 's/^[[:space:]]*-[[:space:]]*\${HOME}\/\([^:]*\):.*/\1/p' "$compose" | sort -u |
while IFS= read -r rel; do
    [[ -z "$rel" ]] && continue
    path="$home/$rel"
    [[ -e "$path" || -L "$path" ]] && continue
    if is_file_source "$rel"; then
        mkdir -p "$(dirname "$path")"
        printf '{}\n' > "$path"
        echo "created file: ~/$rel"
    else
        mkdir -p "$path"
        echo "created directory: ~/$rel"
    fi
done

# SSH refuses keys in a group- or world-readable directory.
[[ -d "$home/.ssh" ]] && chmod 700 "$home/.ssh" 2>/dev/null || true

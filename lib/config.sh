#!/usr/bin/env bash
# ─── CLI config/state (bash-owned) ───────────────────────────────────────────
# The CLI's own remembered state (scan path, selected agent). Kept in a SEPARATE
# file from the backend's config.toml, because the CLI must NEVER write
# config.toml: on Linux/WSL that file lives in the SAME dir the backend uses
# (`~/.config/kronn`) and holds the backend's `encryption_secret` + `auth_token`.
# The old `cat > config.toml` full-clobber there stripped the key and orphaned
# every stored MCP secret (incident 2026-06-30). Contract from now on:
#   • the backend owns  config.toml
#   • the CLI owns       cli.toml
# The backend doesn't even read a top-level `scan_path`/`agent` (it uses the
# structured `[scan]`/`[agents]` tables), so nothing of value is lost by moving
# the CLI's state out of config.toml.

# Absolute path to the CLI-owned state file. Depends on KRONN_CONFIG_DIR, set by
# the main `kronn` script (or by tests) before these functions are called.
kronn_cli_state_file() {
    printf '%s/cli.toml' "${KRONN_CONFIG_DIR}"
}

# Resolve SCAN_PATH from stored CLI state, with a read-only back-compat fallback.
load_config() {
    mkdir -p "$KRONN_CONFIG_DIR"
    SCAN_PATH=""

    local state; state="$(kronn_cli_state_file)"
    if [[ -f "$state" ]]; then
        SCAN_PATH=$(awk -F'"' '/^scan_path/ {print $2}' "$state" 2>/dev/null || true)
    fi

    # Back-compat (READ-ONLY): older CLIs stored scan_path in config.toml. Read a
    # leftover value if present — but NEVER write there (that's the footgun).
    if [[ -z "$SCAN_PATH" && -f "$KRONN_CONFIG_DIR/config.toml" ]]; then
        SCAN_PATH=$(awk -F'"' '/^scan_path/ {print $2}' "$KRONN_CONFIG_DIR/config.toml" 2>/dev/null || true)
    fi

    # Default: parent directory of the kronn repo. A CLI staged in a service
    # release (current/cli, no Makefile) has no such parent: it is the service
    # home. SCAN_PATH stays empty there until ensure_scan_path asks once.
    if [[ -z "$SCAN_PATH" && -f "$KRONN_DIR/Makefile" ]]; then
        SCAN_PATH="$(dirname "${KRONN_DIR}")"
    fi
}

# What a command that cannot ask says when no repositories directory is set.
scan_path_missing_hint() {
    warn "No repositories directory configured."
    printf "  ${DIM}Run ${BOLD}kronn${RESET}${DIM} and choose \"Continue in CLI\" to set it once.${RESET}\n"
}

_scan_path_can_ask() { [[ -t 0 ]]; }

# Interactive commands: ask for the repositories directory once and save it to
# cli.toml. Without a terminal, say so and fail instead of guessing.
ensure_scan_path() {
    [[ -n "$SCAN_PATH" ]] && return 0
    if ! _scan_path_can_ask; then
        scan_path_missing_hint
        return 1
    fi
    local answer dir _
    for _ in 1 2 3; do
        printf "\n${BOLD}Where are your git repositories?${RESET} ${DIM}[%s]${RESET} " "$HOME"
        IFS= read -r answer || return 1
        answer="${answer:-$HOME}"
        # A typed ~ is literal text here: expand it by hand.
        # shellcheck disable=SC2088
        [[ "$answer" == "~" || "$answer" == "~/"* ]] && answer="$HOME${answer#\~}"
        if dir="$(cd "$answer" 2>/dev/null && pwd)"; then
            SCAN_PATH="$dir"
            save_config
            success "Repositories directory saved: $SCAN_PATH"
            return 0
        fi
        warn "Not a directory: $answer"
    done
    return 1
}

# Persist the CLI's chosen scan path + agent — to cli.toml ONLY, never
# config.toml. Atomic temp+rename in the same dir so a crash never leaves a
# half-written file.
save_config() {
    mkdir -p "$KRONN_CONFIG_DIR"
    local state; state="$(kronn_cli_state_file)"
    local tmp="${state}.tmp.$$"
    cat > "$tmp" <<TOML
# Kronn CLI state (managed by ./kronn) — NOT the backend config.
scan_path = "$SCAN_PATH"
agent = "${SELECTED_AGENT:-}"
TOML
    mv -f "$tmp" "$state"
}

#!/usr/bin/env bash
# ─── UI helpers: colors, prompts, interactive menus ──────────────────────────
# Compatible Bash 3.2+ (macOS default)

# Colors — use $'...' so escape sequences are interpreted at assignment time.
# This ensures printf works correctly on all platforms (Bash 3.2+, macOS, Linux, WSL).
RED=$'\033[0;31m'
GREEN=$'\033[0;32m'
YELLOW=$'\033[0;33m'
CYAN=$'\033[0;36m'
BOLD=$'\033[1m'
DIM=$'\033[2m'
RESET=$'\033[0m'

# Cursor control
HIDE_CURSOR=$'\033[?25l'
SHOW_CURSOR=$'\033[?25h'
CLEAR_LINE=$'\033[2K'
MOVE_UP=$'\033[1A'

# ─── Terminal capability detection ──────────────────────────────────────────

# Detect if we can use the fancy interactive menu (arrow keys + cursor control).
# Falls back to numbered input on: Bash < 4, non-interactive terminals, dumb terms.
_INTERACTIVE_MENU=true

_detect_interactive() {
    # Not a terminal
    if [[ ! -t 0 ]] || [[ ! -t 1 ]]; then
        _INTERACTIVE_MENU=false
        return
    fi

    # Dumb terminal
    if [[ "${TERM:-dumb}" == "dumb" ]]; then
        _INTERACTIVE_MENU=false
        return
    fi

    # Test if read -rsn1 actually works by checking bash handles it
    # On Bash 3.2 macOS, read -s may work but cursor movement often doesn't
    if (( BASH_VERSINFO[0] < 4 )); then
        # Quick test: can we move cursor up and back down?
        # Write a test line, move up, clear, move down — if output differs, fallback
        _INTERACTIVE_MENU=false

        # Try to detect if we have a modern bash via brew
        if [[ -x /opt/homebrew/bin/bash ]] || [[ -x /usr/local/bin/bash ]]; then
            local modern_bash
            modern_bash=$(/opt/homebrew/bin/bash --version 2>/dev/null || /usr/local/bin/bash --version 2>/dev/null || true)
            if echo "$modern_bash" | grep -qE 'version [4-9]\.'; then
                # Modern bash is available but not being used
                true
            fi
        fi
    fi
}

_detect_interactive

# ─── Output helpers ──────────────────────────────────────────────────────────

info()    { printf "${CYAN}%s${RESET}\n" "$*"; }
success() { printf "${GREEN}  ✓ %s${RESET}\n" "$*"; }
warn()    { printf "${YELLOW}  ! %s${RESET}\n" "$*"; }
fail()    { printf "${RED}  ✗ %s${RESET}\n" "$*"; }
step()    { printf "\n${BOLD}${CYAN}── %s ──${RESET}\n\n" "$*"; }

banner() {
    local ver
    ver="$(cat "${KRONN_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}/VERSION" 2>/dev/null || echo "0.0.0")"
    ver="${ver%%[[:space:]]}"
    printf "\n"
    printf "  ${CYAN}╭──╮${RESET}\n"
    printf "  ${CYAN}│${YELLOW}⚡${CYAN}│${RESET} ${BOLD}Kronn${RESET} v%s\n" "$ver"
    printf "  ${CYAN}╰──╯${RESET} ${DIM}Enter the grid.${RESET}\n"
    printf "\n"
}

# ─── Key reading ─────────────────────────────────────────────────────────────

# Read a single keypress. Sets KEY to:
#   "up", "down", "enter", "q", or the literal character.
read_key() {
    local byte1 byte2 byte3
    IFS= read -rsn1 byte1 || true
    KEY=""

    if [[ "$byte1" == $'\x1b' ]]; then
        IFS= read -rsn1 -t 1 byte2 2>/dev/null || true
        IFS= read -rsn1 -t 1 byte3 2>/dev/null || true
        case "$byte2$byte3" in
            "[A") KEY="up" ;;
            "[B") KEY="down" ;;
            "[C") KEY="right" ;;
            "[D") KEY="left" ;;
            *)    KEY="escape" ;;
        esac
    elif [[ "$byte1" == "" ]]; then
        KEY="enter"
    elif [[ "$byte1" == "q" || "$byte1" == "Q" ]]; then
        KEY="q"
    elif [[ "$byte1" == "j" ]]; then
        KEY="down"
    elif [[ "$byte1" == "k" ]]; then
        KEY="up"
    else
        KEY="$byte1"
    fi
}

# ─── Interactive menu (arrow keys) ──────────────────────────────────────────

_menu_render() {
    local selected="$1"; shift
    local options=("$@")
    local i

    for i in "${!options[@]}"; do
        printf "${CLEAR_LINE}\r"
        if (( i == selected )); then
            printf "  ${GREEN}▸ ${BOLD}%s${RESET}\n" "${options[$i]}"
        else
            printf "  ${DIM}  %s${RESET}\n" "${options[$i]}"
        fi
    done
}

_menu_move_up() {
    local n="$1" i
    for (( i=0; i<n; i++ )); do
        printf "${MOVE_UP}"
    done
}

_menu_interactive() {
    local title="$1"; shift
    local options=("$@")
    local count=${#options[@]}
    local selected=0

    printf "\n${BOLD}%s${RESET}\n" "$title"
    printf "${DIM}  ↑↓ navigate · enter confirm${RESET}\n\n"

    printf "${HIDE_CURSOR}"
    trap 'printf "${SHOW_CURSOR}"' RETURN

    _menu_render "$selected" "${options[@]}"

    while true; do
        read_key
        case "$KEY" in
            up)
                if (( selected > 0 )); then selected=$((selected - 1)); fi
                ;;
            down)
                if (( selected < count - 1 )); then selected=$((selected + 1)); fi
                ;;
            enter)
                _menu_move_up "$count"
                _menu_render "$selected" "${options[@]}"
                printf "${SHOW_CURSOR}"
                REPLY=$((selected + 1))
                return 0
                ;;
        esac

        _menu_move_up "$count"
        _menu_render "$selected" "${options[@]}"
    done
}

# ─── Fallback menu (numbered input) ─────────────────────────────────────────

_menu_fallback() {
    local title="$1"; shift
    local options=("$@")
    local count=${#options[@]}

    printf "\n${BOLD}%s${RESET}\n\n" "$title"

    local i
    for i in "${!options[@]}"; do
        local num=$((i + 1))
        # Strip ANSI codes for cleaner fallback display
        local clean
        clean=$(printf "%b" "${options[$i]}" | sed 's/\x1b\[[0-9;]*m//g')
        printf "  ${CYAN}%d)${RESET} %s\n" "$num" "$clean"
    done

    echo
    while true; do
        printf "${BOLD}Choice [1-%d]:${RESET} " "$count"
        read -r REPLY
        # Validate input
        if [[ "$REPLY" =~ ^[0-9]+$ ]] && (( REPLY >= 1 && REPLY <= count )); then
            printf "${GREEN}  ▸ %s${RESET}\n" "$(printf "%b" "${options[$((REPLY-1))]}" | sed 's/\x1b\[[0-9;]*m//g')"
            return 0
        fi
        printf "${RED}  Enter a number between 1 and %d${RESET}\n" "$count"
    done
}

# ─── Public API ──────────────────────────────────────────────────────────────

# Interactive menu with arrow keys (or numbered fallback).
# Usage: menu_choice "Title" "Option A" "Option B" "Option C"
# Returns: selected index (1-based) in $REPLY
menu_choice() {
    if [[ "$_INTERACTIVE_MENU" == true ]]; then
        _menu_interactive "$@"
    else
        _menu_fallback "$@"
    fi
}

# Interactive menu with a "Skip" option at the end.
# Returns 0 in $REPLY if user picks "Skip".
menu_choice_or_skip() {
    local title="$1"; shift
    local options=("$@")
    options+=("Skip")

    menu_choice "$title" "${options[@]}"

    if (( REPLY == ${#options[@]} )); then
        REPLY=0
    fi
}

# Interactive yes/no. Returns 0 for yes, 1 for no.
# Usage: ask_yn "Create symlink?" && do_thing
ask_yn() {
    local prompt="$1"

    if [[ "$_INTERACTIVE_MENU" == true ]]; then
        _ask_yn_interactive "$prompt"
    else
        _ask_yn_fallback "$prompt"
    fi
}

_ask_yn_interactive() {
    local prompt="$1"
    local selected=0
    local options=("Yes" "No")

    printf "\n${BOLD}%s${RESET}\n\n" "$prompt"
    printf "${HIDE_CURSOR}"
    trap 'printf "${SHOW_CURSOR}"' RETURN

    _menu_render "$selected" "${options[@]}"

    while true; do
        read_key
        case "$KEY" in
            up|left)
                selected=0
                ;;
            down|right)
                selected=1
                ;;
            enter)
                _menu_move_up 2
                _menu_render "$selected" "${options[@]}"
                printf "${SHOW_CURSOR}"
                return "$selected"
                ;;
            o|O|y|Y)
                _menu_move_up 2
                selected=0
                _menu_render "$selected" "${options[@]}"
                printf "${SHOW_CURSOR}"
                return 0
                ;;
            n|N)
                _menu_move_up 2
                selected=1
                _menu_render "$selected" "${options[@]}"
                printf "${SHOW_CURSOR}"
                return 1
                ;;
        esac

        _menu_move_up 2
        _menu_render "$selected" "${options[@]}"
    done
}

_ask_yn_fallback() {
    local prompt="$1"
    echo
    while true; do
        printf "${BOLD}%s${RESET} ${DIM}(y/n)${RESET} " "$prompt"
        local answer
        # `read` fails on EOF (closed/exhausted stdin — non-interactive runs,
        # pipes, CI). Default to "no" instead of re-prompting forever on an
        # input we can never get.
        if ! read -r answer; then
            echo
            return 1
        fi
        case "$answer" in
            y|Y|yes|YES|o|O|oui|OUI)
                return 0
                ;;
            n|N|no|NO|non|NON)
                return 1
                ;;
            *)
                printf "${RED}  Answer y (yes) or n (no)${RESET}\n"
                ;;
        esac
    done
}

# ─── Platform helpers ────────────────────────────────────────────────────────

# Pure: true (exit 0) when the host OS is macOS — where the Docker stack can't
# execute the user's host CLIs (Darwin binaries can't run in the Linux
# container, and the agents' OAuth creds live in the macOS Keychain, which the
# container can't read). `cmd_web` uses this to warn and point to the native
# path (desktop app / `make dev-backend`). Non-macOS (Linux/WSL) → exit 1 → no
# warning: Docker is the correct path there (host binaries are Linux, run
# directly in the container). Arg overridable for tests.
is_macos_host() {
    [[ "${1:-$(uname -s 2>/dev/null)}" == "Darwin" ]]
}

# Pure: given whether each native-dev tool is present (1 = present, anything
# else = missing), echo the space-separated list of MISSING tool names in a
# stable order (cargo node pnpm), or nothing when all are present. Native dev
# mode (`kronn start-dev`) runs the Rust backend (cargo) + the Vite frontend
# (node + pnpm) on the host with no Docker — these three are the hard
# prerequisites. Kept pure (args, no `command -v`) so it is unit-testable; the
# caller resolves real presence. Cross-platform: native dev is valid on
# Linux/WSL/macOS alike, so there is no OS gating here.
dev_missing_tools() {
    local have_cargo="${1:-0}" have_node="${2:-0}" have_pnpm="${3:-0}"
    local missing=""
    [[ "$have_cargo" == "1" ]] || missing="${missing} cargo"
    [[ "$have_node"  == "1" ]] || missing="${missing} node"
    [[ "$have_pnpm"  == "1" ]] || missing="${missing} pnpm"
    echo "${missing# }"
}

# Print `url` as an OSC 8 terminal hyperlink (clickable in Terminal.app, iTerm2,
# Windows Terminal / WSL, WezTerm, kitty, VS Code…). NOT macOS-specific — gated
# only on stdout being a TTY, so piped/redirected output (CI, logs) gets the
# plain URL with no escape bytes. Prints directly (no trailing newline) so the
# caller controls colors/layout; do NOT wrap in $(...) — that pipe would hide
# the TTY and force the plain fallback. Usage: hyperlink URL [label]
hyperlink() {
    local url="$1" label="${2:-$1}"
    if [[ -t 1 ]]; then
        printf '\033]8;;%s\033\\%s\033]8;;\033\\' "$url" "$label"
    else
        printf '%s' "$label"
    fi
}

# Pure decision for `ensure_in_path`. Inputs (1 = true): is `kronn` already on
# PATH pointing at this checkout, and does the ~/.local/bin symlink already
# exist pointing here. Echoes the action:
#   "ok"     → already reachable; do nothing
#   "adopt"  → symlink exists but ~/.local/bin isn't on PATH; add it for THIS
#              session, silently — do NOT nag to recreate it on every launch
#   "create" → nothing yet; offer to create the symlink
# Split out (pure, no FS) so it's unit-testable; the caller does the FS probes.
path_link_action() {
    local on_path="${1:-0}" link_ok="${2:-0}"
    if [[ "$on_path" == "1" ]]; then
        echo "ok"
    elif [[ "$link_ok" == "1" ]]; then
        echo "adopt"
    else
        echo "create"
    fi
}

# Pure: pick the browser-opener command by availability (1 = present), in
# WSL → Linux → macOS priority. Echoes the command name, or "" if none. Testable.
pick_opener() {
    local has_wslview="${1:-0}" has_xdg="${2:-0}" has_open="${3:-0}"
    if [[ "$has_wslview" == "1" ]]; then
        echo "wslview"
    elif [[ "$has_xdg" == "1" ]]; then
        echo "xdg-open"
    elif [[ "$has_open" == "1" ]]; then
        echo "open"
    else
        echo ""
    fi
}

# Open a URL in the default browser, cross-platform (WSL/Linux/macOS). Returns 1
# when no opener is available so callers can print a manual fallback. Runs the
# opener detached so it never blocks the caller.
open_url() {
    local url="$1" cmd
    cmd=$(pick_opener \
        "$(command -v wslview  >/dev/null 2>&1 && echo 1 || echo 0)" \
        "$(command -v xdg-open >/dev/null 2>&1 && echo 1 || echo 0)" \
        "$(command -v open     >/dev/null 2>&1 && echo 1 || echo 0)")
    [[ -n "$cmd" ]] || return 1
    "$cmd" "$url" >/dev/null 2>&1 &
    return 0
}

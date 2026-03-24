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
    printf "\n"
    printf "  ${CYAN}╭──╮${RESET}\n"
    printf "  ${CYAN}│${YELLOW}⚡${CYAN}│${RESET} ${BOLD}Kronn${RESET} v0.1.0\n"
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
        read -r answer
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

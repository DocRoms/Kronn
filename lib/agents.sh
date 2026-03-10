#!/usr/bin/env bash
# ─── Agent detection and installation ────────────────────────────────────────
# Compatible Bash 3.2+ (macOS default)

# Parallel arrays instead of associative arrays (bash 3 compat)
# Index: 0=claude, 1=codex, 2=vibe, 3=gemini, 4=kiro-cli
_AGENT_NAMES=(claude codex vibe gemini kiro-cli)
_AGENT_ORIGINS=(US US EU US US)
_AGENT_PKGS=("npm:@anthropic-ai/claude-code" "npm:@openai/codex" "pypi:mistral-vibe" "npm:@google/gemini-cli" "curl:kiro-cli")
_AGENT_LABELS=("Claude Code (Anthropic)" "Codex (OpenAI)" "Vibe (Mistral)" "Gemini CLI (Google)" "Kiro (Amazon)")
_AGENT_NODE_MINS=(18 18 0 18 0)

# Detection results (populated by detect_agents)
_AGENT_PATHS=("" "" "" "" "")
_AGENT_VERSIONS=("" "" "" "" "")
_AGENT_LATESTS=("" "" "" "" "")

# ─── Index lookup ─────────────────────────────────────────────────────────────

# Get array index for a given agent name. Returns 1 if not found.
_agent_idx() {
    local name="$1" i
    for i in "${!_AGENT_NAMES[@]}"; do
        if [[ "${_AGENT_NAMES[$i]}" == "$name" ]]; then
            echo "$i"
            return 0
        fi
    done
    return 1
}

# ─── Version helpers ─────────────────────────────────────────────────────────

_parse_version() {
    local raw="$1"
    echo "$raw" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1
}

# Portable timeout wrapper — macOS doesn't have `timeout` by default
_safe_timeout() {
    local duration="$1"; shift
    if command -v timeout >/dev/null 2>&1; then
        timeout "$duration" "$@"
    elif command -v gtimeout >/dev/null 2>&1; then
        # Homebrew coreutils installs as gtimeout
        gtimeout "$duration" "$@"
    else
        "$@"
    fi
}

_get_latest_version() {
    local agent="$1"
    local idx
    idx=$(_agent_idx "$agent") || return
    local pkg="${_AGENT_PKGS[$idx]}"
    [[ -z "$pkg" ]] && return

    local type="${pkg%%:*}"
    local name="${pkg#*:}"

    case "$type" in
        npm)
            _safe_timeout 5 npm view "$name" version 2>/dev/null
            ;;
        pypi)
            _safe_timeout 5 curl -s "https://pypi.org/pypi/$name/json" 2>/dev/null \
                | python3 -c "import sys,json; print(json.load(sys.stdin)['info']['version'])" 2>/dev/null
            ;;
    esac
}

# ─── Detection ───────────────────────────────────────────────────────────────

detect_agents() {
    _AGENT_PATHS=("" "" "" "" "")
    _AGENT_VERSIONS=("" "" "" "" "")
    _AGENT_LATESTS=("" "" "" "" "")

    local i cmd version
    for i in "${!_AGENT_NAMES[@]}"; do
        local name="${_AGENT_NAMES[$i]}"
        cmd=$(command -v "$name" 2>/dev/null || true)
        if [[ -n "$cmd" ]]; then
            version=$(_parse_version "$(_safe_timeout 5 "$name" --version 2>/dev/null | head -1)" || echo "?")
            _AGENT_PATHS[$i]="$cmd"
            _AGENT_VERSIONS[$i]="$version"
        fi
    done
}

# Check for updates on detected agents
check_agent_updates() {
    local i
    for i in "${!_AGENT_NAMES[@]}"; do
        [[ -z "${_AGENT_PATHS[$i]}" ]] && continue
        local latest
        latest=$(_get_latest_version "${_AGENT_NAMES[$i]}")
        if [[ -n "$latest" ]]; then
            _AGENT_LATESTS[$i]="$latest"
        fi
    done
}

# Format agent display line
_format_agent_line() {
    local name="$1"
    local idx
    idx=$(_agent_idx "$name") || return
    local version="${_AGENT_VERSIONS[$idx]}"
    local origin="${_AGENT_ORIGINS[$idx]}"
    local latest="${_AGENT_LATESTS[$idx]}"

    local update_info=""
    if [[ -n "$latest" && "$latest" != "$version" ]]; then
        update_info=" ${YELLOW}⬆ ${latest}${RESET}"
    elif [[ -n "$latest" ]]; then
        update_info=" ${GREEN}✓${RESET}"
    fi

    printf "%s ${DIM}v%s${RESET} ${CYAN}[%s]${RESET}%s\n" "$name" "$version" "$origin" "$update_info"
}

# Count detected agents
_count_detected() {
    local count=0 i
    for i in "${!_AGENT_NAMES[@]}"; do
        [[ -n "${_AGENT_PATHS[$i]}" ]] && count=$((count + 1))
    done
    echo "$count"
}

# Print detected agents
show_detected_agents() {
    local count
    count=$(_count_detected)
    if [[ "$count" -eq 0 ]]; then
        warn "Aucun agent IA detecte."
        return 1
    fi

    info "Agents detectes :"
    check_agent_updates

    local i
    for i in "${!_AGENT_NAMES[@]}"; do
        [[ -z "${_AGENT_PATHS[$i]}" ]] && continue
        success "$(_format_agent_line "${_AGENT_NAMES[$i]}")"
    done
    return 0
}

# ─── Installation helpers ─────────────────────────────────────────────────────

_check_node_version() {
    local agent="$1"
    local idx
    idx=$(_agent_idx "$agent") || return 1
    local min="${_AGENT_NODE_MINS[$idx]}"
    [[ "$min" -eq 0 ]] && return 0

    local node_version
    node_version=$(node --version 2>/dev/null | grep -oE '[0-9]+' | head -1)
    if [[ -z "$node_version" ]]; then
        fail "Node.js non trouve."
        return 1
    fi

    if (( node_version < min )); then
        fail "Node.js >= $min requis (actuel : v${node_version})."
        printf "  ${DIM}Mettre a jour Node.js : https://nodejs.org ou via nvm${RESET}\n"
        return 1
    fi
    return 0
}

_npm_install_global() {
    local package="$1"
    if npm install -g "$package" 2>&1; then
        return 0
    fi
    warn "Permission refusee. Nouvelle tentative avec sudo..."
    sudo npm install -g "$package"
}

# ─── Installation ────────────────────────────────────────────────────────────

install_agent() {
    local agent="$1"
    case "$agent" in
        claude)
            step "Installation de Claude Code"
            if ! command -v npm >/dev/null 2>&1; then
                fail "npm requis pour installer Claude Code"
                printf "  ${DIM}https://docs.anthropic.com/en/docs/claude-code${RESET}\n"
                return 1
            fi
            _check_node_version claude || return 1
            _npm_install_global @anthropic-ai/claude-code
            ;;
        codex)
            step "Installation de Codex"
            if ! command -v npm >/dev/null 2>&1; then
                fail "npm requis pour installer Codex"
                return 1
            fi
            _check_node_version codex || return 1
            _npm_install_global @openai/codex
            ;;
        vibe)
            step "Installation de Vibe (Mistral)"
            if command -v uv >/dev/null 2>&1; then
                uv tool install mistral-vibe
            elif command -v pipx >/dev/null 2>&1; then
                pipx install mistral-vibe
            elif command -v pip3 >/dev/null 2>&1; then
                pip3 install --user mistral-vibe
            else
                fail "uv, pipx ou pip3 requis pour installer Vibe"
                printf "  ${DIM}https://github.com/mistralai/mistral-vibe${RESET}\n"
                return 1
            fi
            ;;
        gemini)
            step "Installation de Gemini CLI (Google)"
            if ! command -v npm >/dev/null 2>&1; then
                fail "npm requis pour installer Gemini CLI"
                printf "  ${DIM}https://github.com/google-gemini/gemini-cli${RESET}\n"
                return 1
            fi
            _check_node_version gemini || return 1
            _npm_install_global @google/gemini-cli
            ;;
        kiro-cli)
            step "Installation de Kiro (Amazon)"
            if ! command -v curl >/dev/null 2>&1; then
                fail "curl requis pour installer Kiro"
                printf "  ${DIM}https://cli.kiro.dev${RESET}\n"
                return 1
            fi
            curl -fsSL https://cli.kiro.dev/install | bash
            ;;
        *)
            fail "Agent inconnu : $agent"
            return 1
            ;;
    esac

    detect_agents
}

uninstall_agent() {
    local agent="$1"
    local idx
    idx=$(_agent_idx "$agent") || { fail "Agent inconnu : $agent"; return 1; }
    local pkg="${_AGENT_PKGS[$idx]}"
    local label="${_AGENT_LABELS[$idx]}"

    local type="${pkg%%:*}"
    local name="${pkg#*:}"

    step "Desinstallation de $label"

    case "$type" in
        npm)
            npm uninstall -g "$name" 2>/dev/null || sudo npm uninstall -g "$name"
            ;;
        pypi)
            if command -v uv >/dev/null 2>&1; then
                uv tool uninstall "$name" 2>/dev/null || true
            elif command -v pipx >/dev/null 2>&1; then
                pipx uninstall "$name" 2>/dev/null || true
            elif command -v pip3 >/dev/null 2>&1; then
                pip3 uninstall -y "$name" 2>/dev/null || true
            fi
            ;;
        curl)
            rm -f "$(command -v "$name" 2>/dev/null)" 2>/dev/null || true
            ;;
    esac

    detect_agents
    if [[ -z "${_AGENT_PATHS[$idx]}" ]]; then
        success "$label desinstalle."
    else
        fail "Echec de la desinstallation."
    fi
}

update_agent() {
    local agent="$1"
    local idx
    idx=$(_agent_idx "$agent") || { fail "Agent inconnu : $agent"; return 1; }
    local pkg="${_AGENT_PKGS[$idx]}"
    local label="${_AGENT_LABELS[$idx]}"

    local type="${pkg%%:*}"
    local name="${pkg#*:}"

    step "Mise a jour de $label"

    case "$type" in
        npm)
            _check_node_version "$agent" || return 1
            _npm_install_global "$name"
            ;;
        pypi)
            if command -v uv >/dev/null 2>&1; then
                uv tool upgrade "$name"
            elif command -v pipx >/dev/null 2>&1; then
                pipx upgrade "$name"
            elif command -v pip3 >/dev/null 2>&1; then
                pip3 install --user --upgrade "$name"
            fi
            ;;
    esac

    detect_agents
    check_agent_updates
    success "$label mis a jour."
}

# ─── Agent management ────────────────────────────────────────────────────────

manage_agents() {
    while true; do
        detect_agents
        check_agent_updates

        echo
        info "Gestion des agents :"
        echo

        local -a options=()
        local -a actions=()

        local i
        for i in "${!_AGENT_NAMES[@]}"; do
            local a="${_AGENT_NAMES[$i]}"
            local label="${_AGENT_LABELS[$i]}"
            local origin="${_AGENT_ORIGINS[$i]}"

            if [[ -n "${_AGENT_PATHS[$i]}" ]]; then
                local ver="${_AGENT_VERSIONS[$i]}"
                local latest="${_AGENT_LATESTS[$i]}"

                if [[ -n "$latest" && "$latest" != "$ver" ]]; then
                    options+=("${YELLOW}⬆${RESET}  ${label} ${DIM}v${ver} → v${latest}${RESET} ${CYAN}[${origin}]${RESET} ${DIM}— mettre a jour${RESET}")
                    actions+=("update:$a")
                fi

                options+=("${RED}✕${RESET}  ${label} ${DIM}v${ver}${RESET} ${CYAN}[${origin}]${RESET} ${DIM}— desinstaller${RESET}")
                actions+=("remove:$a")
            else
                local extra=""
                [[ "$a" == "claude" ]] && extra=" ${DIM}— recommande${RESET}"
                [[ "$a" == "vibe" ]] && extra=" ${DIM}— souverain${RESET}"
                options+=("${GREEN}+${RESET}  ${label} ${CYAN}[${origin}]${RESET}${extra} ${DIM}— installer${RESET}")
                actions+=("install:$a")
            fi
        done

        options+=("${BOLD}Continuer${RESET}")
        actions+=("done")

        menu_choice "Action :" "${options[@]}"
        local action="${actions[$((REPLY-1))]}"
        local action_type="${action%%:*}"
        local action_agent="${action#*:}"

        case "$action_type" in
            install)  install_agent "$action_agent" ;;
            update)   update_agent "$action_agent" ;;
            remove)   uninstall_agent "$action_agent" ;;
            done)     break ;;
        esac
    done
}

# ─── Agent selection flow ────────────────────────────────────────────────────

select_agent() {
    step "Agents IA"

    detect_agents

    if show_detected_agents; then
        if ask_yn "Ajouter / modifier un agent ?"; then
            manage_agents
        fi
    else
        warn "Aucun agent installe. Installation requise."
        manage_agents

        local count
        count=$(_count_detected)
        if [[ "$count" -eq 0 ]]; then
            fail "Aucun agent installe. Installer manuellement puis relancer kronn."
            exit 1
        fi
    fi

    local names=()
    local keys=()
    local i
    for i in "${!_AGENT_NAMES[@]}"; do
        [[ -z "${_AGENT_PATHS[$i]}" ]] && continue
        names+=("$(_format_agent_line "${_AGENT_NAMES[$i]}")")
        keys+=("${_AGENT_NAMES[$i]}")
    done

    if [[ ${#names[@]} -eq 1 ]]; then
        SELECTED_AGENT="${keys[0]}"
        success "Agent principal : $SELECTED_AGENT"
    elif [[ ${#names[@]} -gt 1 ]]; then
        menu_choice "Quel agent principal ?" "${names[@]}"
        SELECTED_AGENT="${keys[$((REPLY-1))]}"
        success "Agent principal : $SELECTED_AGENT"
    fi
}

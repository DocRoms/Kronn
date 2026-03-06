#!/usr/bin/env bash
# ─── Repository scanning and AI context detection ────────────────────────────

# Arrays filled by scan_repos
REPO_PATHS=()
REPO_NAMES=()
REPO_STATUS=()   # human-readable status string per repo

# ─── Scanning ────────────────────────────────────────────────────────────────

# Scan a directory for git repositories (depth 1).
# Usage: scan_repos /path/to/parent
scan_repos() {
    local scan_dir="${1:-.}"
    scan_dir=$(cd "$scan_dir" 2>/dev/null && pwd)

    REPO_PATHS=()
    REPO_NAMES=()
    REPO_STATUS=()

    local dir
    for dir in "$scan_dir"/*/; do
        [[ -d "$dir/.git" ]] || continue

        local name
        name=$(basename "$dir")
        local status
        status=$(detect_ai_context "$dir")

        REPO_PATHS+=("$dir")
        REPO_NAMES+=("$name")
        REPO_STATUS+=("$status")
    done
}

# Detect AI context in a repo. Returns a status string.
detect_ai_context() {
    local repo_dir="$1"
    local parts=()

    # ai/ folder
    if [[ -d "$repo_dir/ai" && -f "$repo_dir/ai/index.md" ]]; then
        parts+=("ai/")
    fi

    # CLAUDE.md or other redirectors
    local redirectors=0
    for f in CLAUDE.md .cursorrules .windsurfrules .clinerules; do
        [[ -f "$repo_dir/$f" ]] && ((redirectors++))
    done
    if (( redirectors > 0 )); then
        parts+=("${redirectors} redirecteurs")
    fi

    # MCP config
    local mcp_count=0
    if [[ -f "$repo_dir/.mcp.json" ]]; then
        mcp_count=$(grep -c '"command"' "$repo_dir/.mcp.json" 2>/dev/null || echo 0)
        parts+=("${mcp_count} MCPs")
    elif [[ -f "$repo_dir/.mcp.json.example" ]]; then
        parts+=("MCP template")
    fi

    # .claude/ settings
    if [[ -d "$repo_dir/.claude" ]]; then
        parts+=(".claude/")
    fi

    if [[ ${#parts[@]} -eq 0 ]]; then
        echo "non configuré"
    else
        local IFS=" + "
        echo "${parts[*]}"
    fi
}

# ─── Display ─────────────────────────────────────────────────────────────────

# Show repo list and let user pick one to configure.
# Sets REPLY to the index (1-based) or 0 to skip.
select_repo() {
    local options=()
    local i

    for i in "${!REPO_NAMES[@]}"; do
        local name="${REPO_NAMES[$i]}"
        local status="${REPO_STATUS[$i]}"

        if [[ "$status" == "non configuré" ]]; then
            options+=("${name} ${DIM}(${status})${RESET}")
        else
            options+=("${name} ${GREEN}(${status})${RESET}")
        fi
    done

    options+=("${CYAN}Lancer l'interface Web${RESET}")
    menu_choice "Depots detectes — choisir celui a configurer :" "${options[@]}"
    # Last option = web interface
    if (( REPLY == ${#options[@]} )); then
        REPLY=0
    fi
}

# ─── Configuration ───────────────────────────────────────────────────────────

# Bootstrap AI context in a repo from templates.
init_repo() {
    local repo_dir="$1"
    local repo_name
    repo_name=$(basename "$repo_dir")
    local template_dir="$KRONN_DIR/templates"

    step "Configuration de $repo_name"

    # Copy ai/ structure if missing or incomplete
    local fresh_ai=false
    if [[ ! -f "$repo_dir/ai/index.md" ]]; then
        info "Copie du template ai/..."
        mkdir -p "$repo_dir/ai"
        # Use rsync if available (portable), fallback to cp without -n (BSD compat)
        if command -v rsync >/dev/null 2>&1; then
            rsync -a --ignore-existing "$template_dir/ai/" "$repo_dir/ai/"
        else
            # BSD cp (macOS) doesn't reliably support -n with -r
            # Copy file by file, skipping existing
            (cd "$template_dir/ai" && find . -type f | while read -r f; do
                if [[ ! -f "$repo_dir/ai/$f" ]]; then
                    mkdir -p "$repo_dir/ai/$(dirname "$f")"
                    cp "$template_dir/ai/$f" "$repo_dir/ai/$f"
                fi
            done)
        fi
        success "ai/ créé"
        fresh_ai=true

        # Inject bootstrap prompt for agent auto-analysis
        inject_bootstrap_prompt "$repo_dir"
    else
        success "ai/ existe déjà"
    fi

    # Copy redirectors if missing
    for f in CLAUDE.md .cursorrules .windsurfrules .clinerules; do
        if [[ ! -f "$repo_dir/$f" && -f "$template_dir/$f" ]]; then
            cp "$template_dir/$f" "$repo_dir/$f"
            success "$f créé"
        fi
    done

    # .cursor/rules
    if [[ -f "$template_dir/.cursor/rules/repo-instructions.mdc" ]]; then
        if [[ ! -f "$repo_dir/.cursor/rules/repo-instructions.mdc" ]]; then
            mkdir -p "$repo_dir/.cursor/rules"
            cp "$template_dir/.cursor/rules/repo-instructions.mdc" "$repo_dir/.cursor/rules/"
            success ".cursor/rules/ créé"
        fi
    fi

    # .github/copilot-instructions.md
    if [[ -f "$template_dir/.github/copilot-instructions.md" ]]; then
        if [[ ! -f "$repo_dir/.github/copilot-instructions.md" ]]; then
            mkdir -p "$repo_dir/.github"
            cp "$template_dir/.github/copilot-instructions.md" "$repo_dir/.github/"
            success ".github/copilot-instructions.md créé"
        fi
    fi

    # MCP template
    if [[ ! -f "$repo_dir/.mcp.json.example" && -f "$template_dir/.mcp.json.example" ]]; then
        cp "$template_dir/.mcp.json.example" "$repo_dir/"
        success ".mcp.json.example créé"
    fi

    if [[ ! -f "$repo_dir/.env.mcp.example" && -f "$template_dir/.env.mcp.example" ]]; then
        cp "$template_dir/.env.mcp.example" "$repo_dir/"
        success ".env.mcp.example créé"
    fi

    # Ensure gitignore has MCP entries
    ensure_gitignore "$repo_dir"

    # Ask about gitignoring ai/
    if [[ "$fresh_ai" == true ]]; then
        echo
        if ask_yn "Ajouter ai/ dans le .gitignore ? (ne pas commiter la doc AI)"; then
            if ! grep -qxF "ai/" "$repo_dir/.gitignore" 2>/dev/null; then
                echo "ai/" >> "$repo_dir/.gitignore"
            fi
            success "ai/ ajouté au .gitignore"
        fi
    fi

    # Sync MCP if secrets exist
    if [[ -f "$KRONN_CONFIG_DIR/secrets.toml" ]]; then
        sync_mcp_for_repo "$repo_dir"
    fi

    echo
    success "Dépôt $repo_name configuré."

    # Propose agent analysis if fresh setup
    if [[ "$fresh_ai" == true ]]; then
        maybe_analyze_repo "$repo_dir"
    fi
}

ensure_gitignore() {
    local repo_dir="$1"
    local gitignore="$repo_dir/.gitignore"
    local entries=(".env.mcp" ".mcp.json" "ai/var/")

    for entry in "${entries[@]}"; do
        if [[ ! -f "$gitignore" ]] || ! grep -qxF "$entry" "$gitignore" 2>/dev/null; then
            echo "$entry" >> "$gitignore"
        fi
    done
}

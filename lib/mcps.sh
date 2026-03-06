#!/usr/bin/env bash
# ─── MCP management and secret sync ─────────────────────────────────────────

KRONN_CONFIG_DIR="${HOME}/.config/kronn"

# ─── Secrets management ─────────────────────────────────────────────────────

# Initialize secrets file if it doesn't exist.
init_secrets() {
    mkdir -p "$KRONN_CONFIG_DIR"
    local secrets_file="$KRONN_CONFIG_DIR/secrets.toml"

    if [[ -f "$secrets_file" ]]; then
        return 0
    fi

    step "Configuration des secrets MCP"
    info "Les tokens sont stockés une seule fois dans :"
    printf "  ${DIM}%s${RESET}\n" "$secrets_file"
    echo

    cat > "$secrets_file" <<'TOML'
# Kronn — MCP Secrets (tokens centralisés)
# Ce fichier est utilisé par `kronn` pour générer les .mcp.json de chaque dépôt.

[atlassian]
url = ""
username = ""
api_token = ""

[github]
personal_access_token = ""

[aws]
access_key_id = ""
secret_access_key = ""
region = "eu-west-1"
TOML

    chmod 600 "$secrets_file"
    success "Fichier secrets créé : $secrets_file"
    warn "Éditer ce fichier avec vos tokens, puis relancer kronn."
    printf "  ${DIM}nano %s${RESET}\n" "$secrets_file"
    echo
}

# Read a value from secrets.toml.
# Usage: secret_get "atlassian" "api_token"
secret_get() {
    local section="$1" key="$2"
    local secrets_file="$KRONN_CONFIG_DIR/secrets.toml"
    [[ -f "$secrets_file" ]] || return 1

    # Simple TOML parser: find [section] then key = "value"
    awk -v section="$section" -v key="$key" '
        /^\[/ { in_section = ($0 == "[" section "]") }
        in_section && $1 == key && $2 == "=" {
            val = $0
            sub(/^[^=]*=[ \t]*"?/, "", val)
            sub(/"[ \t]*$/, "", val)
            print val
            exit
        }
    ' "$secrets_file"
}

# Check if secrets are configured (non-empty).
secrets_configured() {
    local token
    token=$(secret_get "atlassian" "api_token")
    [[ -n "$token" ]] && return 0

    token=$(secret_get "github" "personal_access_token")
    [[ -n "$token" ]] && return 0

    return 1
}

# ─── MCP sync ────────────────────────────────────────────────────────────────

# Generate .mcp.json for a repo from its .mcp.json.example + central secrets.
sync_mcp_for_repo() {
    local repo_dir="$1"
    local template="$repo_dir/.mcp.json.example"
    local output="$repo_dir/.mcp.json"

    if [[ ! -f "$template" ]]; then
        return 1
    fi

    # Export secrets as env vars for envsubst
    export ATLASSIAN_URL
    ATLASSIAN_URL=$(secret_get "atlassian" "url")
    export JIRA_USERNAME
    JIRA_USERNAME=$(secret_get "atlassian" "username")
    export JIRA_API_TOKEN
    JIRA_API_TOKEN=$(secret_get "atlassian" "api_token")
    export CONFLUENCE_USERNAME
    CONFLUENCE_USERNAME=$(secret_get "atlassian" "username")
    export CONFLUENCE_API_TOKEN
    CONFLUENCE_API_TOKEN=$(secret_get "atlassian" "api_token")
    export GITHUB_PERSONAL_ACCESS_TOKEN
    GITHUB_PERSONAL_ACCESS_TOKEN=$(secret_get "github" "personal_access_token")
    export AWS_ACCESS_KEY_ID
    AWS_ACCESS_KEY_ID=$(secret_get "aws" "access_key_id")
    export AWS_SECRET_ACCESS_KEY
    AWS_SECRET_ACCESS_KEY=$(secret_get "aws" "secret_access_key")
    export AWS_REGION
    AWS_REGION=$(secret_get "aws" "region")

    if command -v envsubst >/dev/null 2>&1; then
        envsubst < "$template" > "$output"
        success ".mcp.json généré pour $(basename "$repo_dir")"
    else
        fail "envsubst non trouvé — installer gettext"
        printf "  ${DIM}sudo apt install gettext (Linux) / brew install gettext (macOS)${RESET}\n"
        return 1
    fi
}

# Sync MCP for all known repos.
sync_mcp_all() {
    step "Synchronisation des MCPs"

    if ! secrets_configured; then
        warn "Aucun secret configuré."
        init_secrets
        return 1
    fi

    local synced=0
    for dir in "${REPO_PATHS[@]}"; do
        if [[ -f "$dir/.mcp.json.example" ]]; then
            sync_mcp_for_repo "$dir"
            ((synced++))
        fi
    done

    if (( synced == 0 )); then
        info "Aucun dépôt avec .mcp.json.example trouvé."
    else
        echo
        success "$synced dépôt(s) synchronisé(s)."
    fi
}

# ─── MCP prerequisites check ────────────────────────────────────────────────

check_mcp_prereqs() {
    local ok=true

    if command -v uvx >/dev/null 2>&1; then
        success "uvx trouvé"
    else
        fail "uvx non trouvé — https://docs.astral.sh/uv/"
        ok=false
    fi

    if command -v npx >/dev/null 2>&1; then
        success "npx trouvé"
    else
        fail "npx non trouvé — installer Node.js"
        ok=false
    fi

    if command -v envsubst >/dev/null 2>&1; then
        success "envsubst trouvé"
    else
        fail "envsubst non trouvé — sudo apt install gettext"
        ok=false
    fi

    [[ "$ok" == "true" ]]
}

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

    step "MCP secrets configuration"
    info "Tokens are stored once in:"
    printf "  ${DIM}%s${RESET}\n" "$secrets_file"
    echo

    cat > "$secrets_file" <<'TOML'
# Kronn — MCP Secrets (centralized tokens)
# This file is used by `kronn` to generate .mcp.json for each repository.

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
    success "Secrets file created: $secrets_file"
    warn "Edit this file with your tokens, then relaunch kronn."
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
        # Restrict substitution to known Kronn secrets only — prevent leaking $HOME, $PATH, etc.
        envsubst '$ATLASSIAN_URL $JIRA_USERNAME $JIRA_API_TOKEN $CONFLUENCE_USERNAME $CONFLUENCE_API_TOKEN $GITHUB_PERSONAL_ACCESS_TOKEN $AWS_ACCESS_KEY_ID $AWS_SECRET_ACCESS_KEY $AWS_REGION' < "$template" > "$output"
        success ".mcp.json generated for $(basename "$repo_dir")"
    else
        fail "envsubst not found — install gettext"
        printf "  ${DIM}sudo apt install gettext (Linux) / brew install gettext (macOS)${RESET}\n"
        return 1
    fi
}

# Sync MCP for all known repos.
sync_mcp_all() {
    step "MCP synchronization"

    if ! secrets_configured; then
        warn "No secret configured."
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
        info "No repository with .mcp.json.example found."
    else
        echo
        success "$synced repository(ies) synchronized."
    fi
}

# ─── MCP prerequisites check ────────────────────────────────────────────────

check_mcp_prereqs() {
    local ok=true

    if command -v uvx >/dev/null 2>&1; then
        success "uvx found"
    else
        fail "uvx not found — https://docs.astral.sh/uv/"
        ok=false
    fi

    if command -v npx >/dev/null 2>&1; then
        success "npx found"
    else
        fail "npx not found — install Node.js"
        ok=false
    fi

    if command -v envsubst >/dev/null 2>&1; then
        success "envsubst found"
    else
        fail "envsubst not found — sudo apt install gettext"
        ok=false
    fi

    [[ "$ok" == "true" ]]
}

#!/usr/bin/env bash
# ─── Kronn API client (curl wrapper) ────────────────────────────────────────
# Compatible Bash 3.2+ (macOS default)
#
# When the backend is running, the CLI delegates to its REST API instead of
# duplicating detection/scanning/config logic in shell. If the backend is
# unreachable, every helper returns 1 and the caller falls back to the local
# (shell) code path.
#
# Usage:
#   if kronn_api_available; then
#     agents_json=$(kronn_api GET /agents)
#   fi

# ─── Configuration ──────────────────────────────────────────────────────────

KRONN_API_BASE="${KRONN_API_BASE:-http://localhost:3140}"
KRONN_API_TIMEOUT="${KRONN_API_TIMEOUT:-3}"

# Auth token (if the user activated Bearer auth from the UI).
# Read from the same localStorage-sync file the frontend uses.
_kronn_auth_header() {
    local token_file="${HOME}/.config/kronn/.auth_token"
    if [[ -f "$token_file" ]]; then
        local token
        token=$(cat "$token_file" 2>/dev/null)
        if [[ -n "$token" ]]; then
            echo "Authorization: Bearer $token"
            return
        fi
    fi
    echo ""
}

# ─── Availability probe ─────────────────────────────────────────────────────

# Returns 0 if the backend is reachable, 1 otherwise.
# Result is cached for the lifetime of the script (no repeated probes).
_KRONN_API_AVAILABLE=""
kronn_api_available() {
    if [[ -n "$_KRONN_API_AVAILABLE" ]]; then
        [[ "$_KRONN_API_AVAILABLE" == "1" ]]
        return
    fi

    local health
    health=$(curl -sf --connect-timeout "$KRONN_API_TIMEOUT" \
        "${KRONN_API_BASE}/api/health" 2>/dev/null) || {
        _KRONN_API_AVAILABLE="0"
        return 1
    }

    # Sanity: make sure we got a JSON-ish response, not an HTML error page
    if echo "$health" | grep -q '"ok"'; then
        _KRONN_API_AVAILABLE="1"
        return 0
    fi

    _KRONN_API_AVAILABLE="0"
    return 1
}

# Reset the cache — useful after `kronn start` brings the backend up.
kronn_api_reset_cache() {
    _KRONN_API_AVAILABLE=""
}

# ─── Generic API call ────────────────────────────────────────────────────────

# kronn_api METHOD /path [json_body]
# Prints the raw JSON response on stdout. Returns 1 on any error.
kronn_api() {
    local method="$1" path="$2" body="${3:-}"
    local url="${KRONN_API_BASE}/api${path}"
    local auth
    auth=$(_kronn_auth_header)

    local -a curl_args=(
        -sf
        --connect-timeout "$KRONN_API_TIMEOUT"
        -X "$method"
        -H "Content-Type: application/json"
    )
    [[ -n "$auth" ]] && curl_args+=(-H "$auth")
    [[ -n "$body" ]] && curl_args+=(-d "$body")

    curl "${curl_args[@]}" "$url" 2>/dev/null
}

# Convenience: extract `.data` from an ApiResponse<T> JSON.
# Returns the inner payload or empty string on error.
kronn_api_data() {
    local json
    json=$(kronn_api "$@") || return 1
    echo "$json" | _json_extract_data
}

# Minimal JSON `.data` extractor — works without jq via Python fallback.
# Preference: jq (fast, common) → python3 (always on macOS/Linux).
_json_extract_data() {
    if command -v jq >/dev/null 2>&1; then
        jq -r '.data // empty'
    elif command -v python3 >/dev/null 2>&1; then
        python3 -c "
import sys, json
try:
    d = json.load(sys.stdin)
    v = d.get('data')
    if v is not None:
        print(json.dumps(v) if isinstance(v, (dict, list)) else str(v))
except: pass
"
    else
        # Last resort: cat through (caller gets the full ApiResponse)
        cat
    fi
}

# ─── Formatted helpers for common commands ───────────────────────────────────

# Print detected agents in a human-friendly table.
# Uses the same colors/layout as the local `show_detected_agents` but
# sources data from GET /api/agents (instant, always up-to-date).
kronn_api_show_agents() {
    local json
    json=$(kronn_api_data GET /agents) || return 1

    # Parse with jq if available, python3 fallback
    if command -v jq >/dev/null 2>&1; then
        echo "$json" | jq -r '.[] |
            (if .installed then "  \u001b[32m✓\u001b[0m " else (if .runtime_available then "  \u001b[33m~\u001b[0m " else "  \u001b[2m✗\u001b[0m " end) end)
            + .name + " "
            + (if .version then "\u001b[2mv" + .version + "\u001b[0m " else "" end)
            + "\u001b[36m[" + .origin + "]\u001b[0m"
            + (if .installed then "" else (if .runtime_available then " \u001b[2m(npx)\u001b[0m" else " \u001b[2m— not installed\u001b[0m" end) end)
        '
    elif command -v python3 >/dev/null 2>&1; then
        echo "$json" | python3 -c "
import sys, json
G='\033[32m'; Y='\033[33m'; C='\033[36m'; D='\033[2m'; R='\033[0m'
for a in json.load(sys.stdin):
    if a['installed']:
        mark = f'  {G}✓{R} '
    elif a.get('runtime_available'):
        mark = f'  {Y}~{R} '
    else:
        mark = f'  {D}✗{R} '
    ver = f' {D}v{a[\"version\"]}{R}' if a.get('version') else ''
    origin = f' {C}[{a[\"origin\"]}]{R}'
    suffix = ''
    if not a['installed']:
        suffix = f' {D}(npx){R}' if a.get('runtime_available') else f' {D}— not installed{R}'
    print(f'{mark}{a[\"name\"]}{ver}{origin}{suffix}')
"
    else
        echo "$json"
    fi
}

# Print Kronn version + host info from /api/health.
kronn_api_show_health() {
    local json
    json=$(curl -sf --connect-timeout "$KRONN_API_TIMEOUT" \
        "${KRONN_API_BASE}/api/health" 2>/dev/null) || return 1

    if command -v jq >/dev/null 2>&1; then
        local ver host_os
        ver=$(echo "$json" | jq -r '.version // "?"')
        host_os=$(echo "$json" | jq -r '.host_os // "?"')
        printf "  Kronn v%s — %s\n" "$ver" "$host_os"
    else
        echo "$json"
    fi
}

# Print project count + discussion count + DB size from API.
kronn_api_show_status() {
    local db_json agents_json
    db_json=$(kronn_api_data GET /config/db-info) || return 1
    agents_json=$(kronn_api_data GET /agents) || return 1

    if command -v jq >/dev/null 2>&1; then
        local projects discs msgs
        projects=$(echo "$db_json" | jq '.project_count // 0')
        discs=$(echo "$db_json" | jq '.discussion_count // 0')
        msgs=$(echo "$db_json" | jq '.message_count // 0')
        local installed
        installed=$(echo "$agents_json" | jq '[.[] | select(.installed)] | length')

        printf "  Projects: %s  |  Discussions: %s  |  Messages: %s\n" "$projects" "$discs" "$msgs"
        printf "  Agents installed: %s\n" "$installed"
    else
        echo "  DB: $db_json"
    fi
}

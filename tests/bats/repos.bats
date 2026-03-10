#!/usr/bin/env bats
# ─── Tests for lib/repos.sh ──────────────────────────────────────────────────

load test_helper

setup() {
    _load_lib "ui.sh"
    _load_lib "repos.sh"
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-repos-test-XXXXXX)"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# ─── scan_repos ──────────────────────────────────────────────────────────────

@test "scan_repos: finds git repos at depth 1" {
    mkdir -p "$TEST_TMPDIR/parent/repo-a/.git"
    mkdir -p "$TEST_TMPDIR/parent/repo-b/.git"
    mkdir -p "$TEST_TMPDIR/parent/not-a-repo"

    scan_repos "$TEST_TMPDIR/parent"
    [ "${#REPO_PATHS[@]}" -eq 2 ]
}

@test "scan_repos: populates REPO_NAMES" {
    mkdir -p "$TEST_TMPDIR/parent/my-project/.git"

    scan_repos "$TEST_TMPDIR/parent"
    [ "${#REPO_NAMES[@]}" -eq 1 ]
    [ "${REPO_NAMES[0]}" = "my-project" ]
}

@test "scan_repos: populates REPO_STATUS" {
    mkdir -p "$TEST_TMPDIR/parent/configured/.git"
    mkdir -p "$TEST_TMPDIR/parent/configured/ai"
    echo "# Index" > "$TEST_TMPDIR/parent/configured/ai/index.md"

    scan_repos "$TEST_TMPDIR/parent"
    [[ "${REPO_STATUS[0]}" == *"ai/"* ]]
}

@test "scan_repos: returns empty arrays for directory with no repos" {
    mkdir -p "$TEST_TMPDIR/empty"

    scan_repos "$TEST_TMPDIR/empty"
    [ "${#REPO_PATHS[@]}" -eq 0 ]
    [ "${#REPO_NAMES[@]}" -eq 0 ]
    [ "${#REPO_STATUS[@]}" -eq 0 ]
}

@test "scan_repos: ignores nested repos (only depth 1)" {
    mkdir -p "$TEST_TMPDIR/parent/outer/.git"
    mkdir -p "$TEST_TMPDIR/parent/outer/inner/.git"

    scan_repos "$TEST_TMPDIR/parent"
    [ "${#REPO_PATHS[@]}" -eq 1 ]
    [ "${REPO_NAMES[0]}" = "outer" ]
}

@test "scan_repos: resets arrays on each call" {
    mkdir -p "$TEST_TMPDIR/parent1/repo-a/.git"
    mkdir -p "$TEST_TMPDIR/parent2/repo-b/.git"
    mkdir -p "$TEST_TMPDIR/parent2/repo-c/.git"

    scan_repos "$TEST_TMPDIR/parent1"
    [ "${#REPO_PATHS[@]}" -eq 1 ]

    scan_repos "$TEST_TMPDIR/parent2"
    [ "${#REPO_PATHS[@]}" -eq 2 ]
}

@test "scan_repos: defaults to current directory when no arg given" {
    mkdir -p "$TEST_TMPDIR/workdir/myrepo/.git"

    cd "$TEST_TMPDIR/workdir"
    scan_repos
    [ "${#REPO_PATHS[@]}" -eq 1 ]
}

# ─── detect_ai_context ──────────────────────────────────────────────────────

@test "detect_ai_context: reports 'non configure' for empty repo" {
    mkdir -p "$TEST_TMPDIR/repo"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output "non configuré"
}

@test "detect_ai_context: detects ai/ directory" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Index" > "$TEST_TMPDIR/repo/ai/index.md"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "ai/"
}

@test "detect_ai_context: requires ai/index.md (not just ai/ directory)" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    # No index.md inside

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output "non configuré"
}

@test "detect_ai_context: detects CLAUDE.md redirector" {
    mkdir -p "$TEST_TMPDIR/repo"
    echo "Read ai/index.md" > "$TEST_TMPDIR/repo/CLAUDE.md"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "1 redirecteurs"
}

@test "detect_ai_context: counts multiple redirectors" {
    mkdir -p "$TEST_TMPDIR/repo"
    echo "x" > "$TEST_TMPDIR/repo/CLAUDE.md"
    echo "x" > "$TEST_TMPDIR/repo/.cursorrules"
    echo "x" > "$TEST_TMPDIR/repo/.windsurfrules"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "3 redirecteurs"
}

@test "detect_ai_context: detects all 4 redirectors" {
    mkdir -p "$TEST_TMPDIR/repo"
    echo "x" > "$TEST_TMPDIR/repo/CLAUDE.md"
    echo "x" > "$TEST_TMPDIR/repo/.cursorrules"
    echo "x" > "$TEST_TMPDIR/repo/.windsurfrules"
    echo "x" > "$TEST_TMPDIR/repo/.clinerules"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "4 redirecteurs"
}

@test "detect_ai_context: detects MCP config with server count" {
    mkdir -p "$TEST_TMPDIR/repo"
    cat > "$TEST_TMPDIR/repo/.mcp.json" <<'JSON'
{
  "mcpServers": {
    "github": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-github"] },
    "slack": { "command": "npx", "args": ["-y", "@modelcontextprotocol/server-slack"] }
  }
}
JSON

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "2 MCPs"
}

@test "detect_ai_context: detects MCP template" {
    mkdir -p "$TEST_TMPDIR/repo"
    echo '{}' > "$TEST_TMPDIR/repo/.mcp.json.example"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "MCP template"
}

@test "detect_ai_context: prefers .mcp.json over .mcp.json.example" {
    mkdir -p "$TEST_TMPDIR/repo"
    echo '{"mcpServers": {"gh": {"command": "npx"}}}' > "$TEST_TMPDIR/repo/.mcp.json"
    echo '{}' > "$TEST_TMPDIR/repo/.mcp.json.example"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "1 MCPs"
    refute_output --partial "MCP template"
}

@test "detect_ai_context: detects .claude/ directory" {
    mkdir -p "$TEST_TMPDIR/repo/.claude"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial ".claude/"
}

@test "detect_ai_context: combines all signals with + separator" {
    mkdir -p "$TEST_TMPDIR/repo/ai" "$TEST_TMPDIR/repo/.claude"
    echo "# Index" > "$TEST_TMPDIR/repo/ai/index.md"
    echo "x" > "$TEST_TMPDIR/repo/CLAUDE.md"
    echo '{"mcpServers":{"gh":{"command":"npx"}}}' > "$TEST_TMPDIR/repo/.mcp.json"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "ai/"
    assert_output --partial "1 redirecteurs"
    assert_output --partial "1 MCPs"
    assert_output --partial ".claude/"
    assert_output --partial "+"
}

# ─── ensure_gitignore ────────────────────────────────────────────────────────

@test "ensure_gitignore: creates entries in new .gitignore" {
    mkdir -p "$TEST_TMPDIR/repo"

    ensure_gitignore "$TEST_TMPDIR/repo"

    local gitignore="$TEST_TMPDIR/repo/.gitignore"
    [ -f "$gitignore" ]
    run grep -xF ".env.mcp" "$gitignore"
    assert_success
    run grep -xF ".mcp.json" "$gitignore"
    assert_success
    run grep -xF "ai/var/" "$gitignore"
    assert_success
}

@test "ensure_gitignore: idempotent — no duplicate entries" {
    mkdir -p "$TEST_TMPDIR/repo"

    ensure_gitignore "$TEST_TMPDIR/repo"
    ensure_gitignore "$TEST_TMPDIR/repo"

    local count
    count=$(grep -cxF ".mcp.json" "$TEST_TMPDIR/repo/.gitignore")
    [ "$count" -eq 1 ]
}

@test "ensure_gitignore: preserves existing content" {
    mkdir -p "$TEST_TMPDIR/repo"
    echo "node_modules/" > "$TEST_TMPDIR/repo/.gitignore"

    ensure_gitignore "$TEST_TMPDIR/repo"

    run grep -xF "node_modules/" "$TEST_TMPDIR/repo/.gitignore"
    assert_success
    run grep -xF ".mcp.json" "$TEST_TMPDIR/repo/.gitignore"
    assert_success
}

@test "ensure_gitignore: fails when repo directory does not exist" {
    run ensure_gitignore "$TEST_TMPDIR/nonexistent-repo"
    assert_failure
}

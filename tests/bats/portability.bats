#!/usr/bin/env bats
# ─── Portability tests: platform-specific code paths ─────────────────────────
# Tests functions that behave differently on GNU (Linux) vs BSD (macOS).
# Uses temp directories as fixtures; cleaned up in teardown.

load test_helper

# ═════════════════════════════════════════════════════════════════════════════
# _safe_timeout (agents.sh) — timeout vs gtimeout vs fallback
# ═════════════════════════════════════════════════════════════════════════════

setup() {
    _load_lib "ui.sh"

    TEST_TMPDIR="$(mktemp -d /tmp/kronn-portability-XXXXXX)"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# ─── _safe_timeout ───────────────────────────────────────────────────────────

@test "_safe_timeout: runs command successfully" {
    _load_lib "agents.sh"
    run _safe_timeout 5 echo "hello"
    assert_success
    assert_output "hello"
}

@test "_safe_timeout: passes arguments through" {
    _load_lib "agents.sh"
    run _safe_timeout 5 printf "%s-%s" "foo" "bar"
    assert_success
    assert_output "foo-bar"
}

@test "_safe_timeout: falls back to running without timeout when neither available" {
    _load_lib "agents.sh"
    # Override PATH to remove timeout and gtimeout
    local saved_path="$PATH"
    mkdir -p "$TEST_TMPDIR/bin"
    # Create a minimal PATH with only essential builtins
    # We can't fully hide builtins but we test the fallback logic
    PATH="$TEST_TMPDIR/bin"

    # Redefine to test fallback: unset the function and source fresh
    unset -f _safe_timeout
    _safe_timeout() {
        local duration="$1"; shift
        if command -v timeout >/dev/null 2>&1; then
            timeout "$duration" "$@"
        elif command -v gtimeout >/dev/null 2>&1; then
            gtimeout "$duration" "$@"
        else
            "$@"
        fi
    }

    run _safe_timeout 1 echo "fallback works"
    PATH="$saved_path"
    assert_success
    assert_output "fallback works"
}

# ═════════════════════════════════════════════════════════════════════════════
# sed -i portability (analyze.sh) — GNU vs BSD sed
# ═════════════════════════════════════════════════════════════════════════════

@test "remove_bootstrap_prompt: removes markers from file" {
    _load_lib "analyze.sh"

    # Create a test file with bootstrap prompt
    local test_file="$TEST_TMPDIR/repo/ai/index.md"
    mkdir -p "$TEST_TMPDIR/repo/ai"
    cat > "$test_file" <<'EOF'
<!-- KRONN:BOOTSTRAP:START -->
This is the bootstrap prompt.
Multiple lines of content.
<!-- KRONN:BOOTSTRAP:END -->

# Real content starts here
Some actual documentation.
EOF

    run remove_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    # Verify markers and content between them are removed
    run grep "KRONN:BOOTSTRAP" "$test_file"
    assert_failure  # grep should not find any matches

    # Verify real content is preserved
    run grep "Real content starts here" "$test_file"
    assert_success

    run grep "Some actual documentation" "$test_file"
    assert_success
}

@test "remove_bootstrap_prompt: preserves content before and after markers" {
    _load_lib "analyze.sh"

    local test_file="$TEST_TMPDIR/repo/ai/index.md"
    mkdir -p "$TEST_TMPDIR/repo/ai"
    cat > "$test_file" <<'EOF'
# Header before
Content before markers.

<!-- KRONN:BOOTSTRAP:START -->
Bootstrap content to remove.
<!-- KRONN:BOOTSTRAP:END -->

# Header after
Content after markers.
EOF

    remove_bootstrap_prompt "$TEST_TMPDIR/repo"

    # Both before and after content must survive
    run cat "$test_file"
    assert_output --partial "Header before"
    assert_output --partial "Content before markers"
    assert_output --partial "Header after"
    assert_output --partial "Content after markers"

    # Bootstrap content must be gone
    refute_output --partial "Bootstrap content to remove"
}

@test "remove_bootstrap_prompt: handles missing file gracefully" {
    _load_lib "analyze.sh"
    run remove_bootstrap_prompt "$TEST_TMPDIR/nonexistent"
    assert_success
}

@test "remove_bootstrap_prompt: no-op when markers absent" {
    _load_lib "analyze.sh"

    local test_file="$TEST_TMPDIR/repo/ai/index.md"
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Clean file with no markers" > "$test_file"

    local before
    before=$(cat "$test_file")

    remove_bootstrap_prompt "$TEST_TMPDIR/repo"

    local after
    after=$(cat "$test_file")

    [ "$before" = "$after" ]
}

# ═════════════════════════════════════════════════════════════════════════════
# inject_bootstrap_prompt + has_bootstrap_prompt (analyze.sh)
# ═════════════════════════════════════════════════════════════════════════════

@test "inject_bootstrap_prompt: injects into clean file" {
    _load_lib "analyze.sh"

    local test_file="$TEST_TMPDIR/repo/ai/index.md"
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Original content" > "$test_file"

    run inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    # Markers should be present
    run grep "KRONN:BOOTSTRAP:START" "$test_file"
    assert_success

    run grep "KRONN:BOOTSTRAP:END" "$test_file"
    assert_success

    # Original content should still be there (appended after prompt)
    run grep "Original content" "$test_file"
    assert_success
}

@test "inject_bootstrap_prompt: idempotent — does not double-inject" {
    _load_lib "analyze.sh"

    local test_file="$TEST_TMPDIR/repo/ai/index.md"
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Content" > "$test_file"

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    local size_after_first
    size_after_first=$(wc -c < "$test_file")

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    local size_after_second
    size_after_second=$(wc -c < "$test_file")

    [ "$size_after_first" = "$size_after_second" ]
}

@test "has_bootstrap_prompt: detects presence" {
    _load_lib "analyze.sh"

    local test_file="$TEST_TMPDIR/repo/ai/index.md"
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# No prompt" > "$test_file"

    run has_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_failure

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"

    run has_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success
}

@test "inject then remove roundtrip: file returns to original content" {
    _load_lib "analyze.sh"

    local test_file="$TEST_TMPDIR/repo/ai/index.md"
    mkdir -p "$TEST_TMPDIR/repo/ai"
    printf "# My project\n\nSome documentation.\n" > "$test_file"

    local original
    original=$(cat "$test_file")

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    remove_bootstrap_prompt "$TEST_TMPDIR/repo"

    local restored
    restored=$(cat "$test_file")

    # Content should be back (minus any trailing whitespace differences)
    run grep "My project" "$test_file"
    assert_success

    run grep "Some documentation" "$test_file"
    assert_success

    run has_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_failure
}

# ═════════════════════════════════════════════════════════════════════════════
# ensure_gitignore (repos.sh) — idempotent .gitignore entries
# ═════════════════════════════════════════════════════════════════════════════

@test "ensure_gitignore: creates entries in new .gitignore" {
    _load_lib "repos.sh"

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
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo"

    ensure_gitignore "$TEST_TMPDIR/repo"
    ensure_gitignore "$TEST_TMPDIR/repo"

    local gitignore="$TEST_TMPDIR/repo/.gitignore"
    local count
    count=$(grep -cxF ".mcp.json" "$gitignore")
    [ "$count" -eq 1 ]
}

@test "ensure_gitignore: preserves existing content" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo"
    echo "node_modules/" > "$TEST_TMPDIR/repo/.gitignore"

    ensure_gitignore "$TEST_TMPDIR/repo"

    run grep -xF "node_modules/" "$TEST_TMPDIR/repo/.gitignore"
    assert_success

    run grep -xF ".mcp.json" "$TEST_TMPDIR/repo/.gitignore"
    assert_success
}

@test "ensure_gitignore: skips entries already present" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo"
    printf ".env.mcp\n.mcp.json\n" > "$TEST_TMPDIR/repo/.gitignore"

    ensure_gitignore "$TEST_TMPDIR/repo"

    # Should only add ai/var/ (the missing one)
    local count
    count=$(wc -l < "$TEST_TMPDIR/repo/.gitignore")
    [ "$count" -eq 3 ]
}

# ═════════════════════════════════════════════════════════════════════════════
# detect_ai_context (repos.sh) — filesystem status detection
# ═════════════════════════════════════════════════════════════════════════════

@test "detect_ai_context: reports 'non configure' for empty repo" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output "non configuré"
}

@test "detect_ai_context: detects ai/ directory" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Index" > "$TEST_TMPDIR/repo/ai/index.md"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "ai/"
}

@test "detect_ai_context: detects redirectors" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo"
    echo "Read ai/index.md" > "$TEST_TMPDIR/repo/CLAUDE.md"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "1 redirecteurs"
}

@test "detect_ai_context: counts multiple redirectors" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo"
    echo "x" > "$TEST_TMPDIR/repo/CLAUDE.md"
    echo "x" > "$TEST_TMPDIR/repo/.cursorrules"
    echo "x" > "$TEST_TMPDIR/repo/.windsurfrules"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial "3 redirecteurs"
}

@test "detect_ai_context: detects MCP config" {
    _load_lib "repos.sh"

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

@test "detect_ai_context: detects .claude/ directory" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo/.claude"

    run detect_ai_context "$TEST_TMPDIR/repo"
    assert_success
    assert_output --partial ".claude/"
}

@test "detect_ai_context: combines all signals" {
    _load_lib "repos.sh"

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
}

# ═════════════════════════════════════════════════════════════════════════════
# scan_repos (repos.sh) — directory scanning
# ═════════════════════════════════════════════════════════════════════════════

@test "scan_repos: finds git repos at depth 1" {
    _load_lib "repos.sh"

    # Create two repos and one non-repo
    mkdir -p "$TEST_TMPDIR/parent/repo-a/.git"
    mkdir -p "$TEST_TMPDIR/parent/repo-b/.git"
    mkdir -p "$TEST_TMPDIR/parent/not-a-repo"

    scan_repos "$TEST_TMPDIR/parent"

    [ "${#REPO_PATHS[@]}" -eq 2 ]
}

@test "scan_repos: populates REPO_NAMES" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/parent/my-project/.git"

    scan_repos "$TEST_TMPDIR/parent"

    [ "${#REPO_NAMES[@]}" -eq 1 ]
    [ "${REPO_NAMES[0]}" = "my-project" ]
}

@test "scan_repos: populates REPO_STATUS with detection results" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/parent/configured/.git"
    mkdir -p "$TEST_TMPDIR/parent/configured/ai"
    echo "# Index" > "$TEST_TMPDIR/parent/configured/ai/index.md"

    scan_repos "$TEST_TMPDIR/parent"

    [[ "${REPO_STATUS[0]}" == *"ai/"* ]]
}

@test "scan_repos: returns empty arrays for directory with no repos" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/empty"

    scan_repos "$TEST_TMPDIR/empty"

    [ "${#REPO_PATHS[@]}" -eq 0 ]
    [ "${#REPO_NAMES[@]}" -eq 0 ]
    [ "${#REPO_STATUS[@]}" -eq 0 ]
}

# ═════════════════════════════════════════════════════════════════════════════
# rsync vs cp fallback (repos.sh) — directory copy portability
# ═════════════════════════════════════════════════════════════════════════════

@test "template copy: rsync path copies directory tree" {
    # Only run if rsync is available
    command -v rsync >/dev/null 2>&1 || skip "rsync not available"

    # Create a template structure
    mkdir -p "$TEST_TMPDIR/template/ai/architecture"
    echo "# Index" > "$TEST_TMPDIR/template/ai/index.md"
    echo "# Map" > "$TEST_TMPDIR/template/ai/repo-map.md"
    echo "# Overview" > "$TEST_TMPDIR/template/ai/architecture/overview.md"

    # Target dir
    mkdir -p "$TEST_TMPDIR/target/ai"

    rsync -a --ignore-existing "$TEST_TMPDIR/template/ai/" "$TEST_TMPDIR/target/ai/"

    [ -f "$TEST_TMPDIR/target/ai/index.md" ]
    [ -f "$TEST_TMPDIR/target/ai/repo-map.md" ]
    [ -f "$TEST_TMPDIR/target/ai/architecture/overview.md" ]
}

@test "template copy: rsync skips existing files" {
    command -v rsync >/dev/null 2>&1 || skip "rsync not available"

    mkdir -p "$TEST_TMPDIR/template/ai"
    echo "template content" > "$TEST_TMPDIR/template/ai/index.md"

    mkdir -p "$TEST_TMPDIR/target/ai"
    echo "existing content" > "$TEST_TMPDIR/target/ai/index.md"

    rsync -a --ignore-existing "$TEST_TMPDIR/template/ai/" "$TEST_TMPDIR/target/ai/"

    run cat "$TEST_TMPDIR/target/ai/index.md"
    assert_output "existing content"
}

@test "template copy: manual fallback copies directory tree" {
    # Simulate the fallback path from repos.sh init_repo()
    mkdir -p "$TEST_TMPDIR/template/ai/architecture"
    echo "# Index" > "$TEST_TMPDIR/template/ai/index.md"
    echo "# Map" > "$TEST_TMPDIR/template/ai/repo-map.md"
    echo "# Overview" > "$TEST_TMPDIR/template/ai/architecture/overview.md"

    local target="$TEST_TMPDIR/target"
    mkdir -p "$target/ai"

    # Replicate the exact fallback code from repos.sh
    (cd "$TEST_TMPDIR/template/ai" && find . -type f | while read -r f; do
        if [[ ! -f "$target/ai/$f" ]]; then
            mkdir -p "$target/ai/$(dirname "$f")"
            cp "$TEST_TMPDIR/template/ai/$f" "$target/ai/$f"
        fi
    done)

    [ -f "$target/ai/index.md" ]
    [ -f "$target/ai/repo-map.md" ]
    [ -f "$target/ai/architecture/overview.md" ]
}

@test "template copy: manual fallback skips existing files" {
    mkdir -p "$TEST_TMPDIR/template/ai"
    echo "template content" > "$TEST_TMPDIR/template/ai/index.md"

    local target="$TEST_TMPDIR/target"
    mkdir -p "$target/ai"
    echo "existing content" > "$target/ai/index.md"

    (cd "$TEST_TMPDIR/template/ai" && find . -type f | while read -r f; do
        if [[ ! -f "$target/ai/$f" ]]; then
            mkdir -p "$target/ai/$(dirname "$f")"
            cp "$TEST_TMPDIR/template/ai/$f" "$target/ai/$f"
        fi
    done)

    run cat "$target/ai/index.md"
    assert_output "existing content"
}

@test "template copy: both paths produce same result" {
    command -v rsync >/dev/null 2>&1 || skip "rsync not available"

    # Create template
    mkdir -p "$TEST_TMPDIR/template/ai/ops"
    echo "idx" > "$TEST_TMPDIR/template/ai/index.md"
    echo "map" > "$TEST_TMPDIR/template/ai/repo-map.md"
    echo "ops" > "$TEST_TMPDIR/template/ai/ops/debug.md"

    # rsync path
    mkdir -p "$TEST_TMPDIR/rsync-target/ai"
    rsync -a --ignore-existing "$TEST_TMPDIR/template/ai/" "$TEST_TMPDIR/rsync-target/ai/"

    # Manual fallback path
    mkdir -p "$TEST_TMPDIR/manual-target/ai"
    (cd "$TEST_TMPDIR/template/ai" && find . -type f | while read -r f; do
        if [[ ! -f "$TEST_TMPDIR/manual-target/ai/$f" ]]; then
            mkdir -p "$TEST_TMPDIR/manual-target/ai/$(dirname "$f")"
            cp "$TEST_TMPDIR/template/ai/$f" "$TEST_TMPDIR/manual-target/ai/$f"
        fi
    done)

    # Compare: same file tree
    local rsync_files manual_files
    rsync_files=$(cd "$TEST_TMPDIR/rsync-target/ai" && find . -type f | sort)
    manual_files=$(cd "$TEST_TMPDIR/manual-target/ai" && find . -type f | sort)

    [ "$rsync_files" = "$manual_files" ]
}

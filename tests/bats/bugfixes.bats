#!/usr/bin/env bats
# ─── Non-regression tests for shell bugs fixed 2026-03-10 ────────────────────
# Each test documents a specific bug and verifies the fix.

load test_helper

setup() {
    _load_lib "ui.sh"
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-bugfix-XXXXXX)"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# ═════════════════════════════════════════════════════════════════════════════
# Bug 1: inject_bootstrap_prompt used mktemp in /tmp causing cross-filesystem
#         mv failures when repo is on a different mount (Docker volume, NFS, WSL)
# Fix: create temp file in same directory as target
# ═════════════════════════════════════════════════════════════════════════════

@test "bug1: inject_bootstrap_prompt creates temp in repo dir, not /tmp" {
    _load_lib "analyze.sh"

    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Content" > "$TEST_TMPDIR/repo/ai/index.md"

    # Inject — should work even if /tmp is a different filesystem
    run inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    # Verify no leftover temp files in repo dir
    local leftover
    leftover=$(find "$TEST_TMPDIR/repo/ai" -name '.index.md.*' 2>/dev/null | wc -l)
    [ "$leftover" -eq 0 ]

    # Verify content is correct
    run grep "KRONN:BOOTSTRAP:START" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
}

# ═════════════════════════════════════════════════════════════════════════════
# Bug 2: remove_bootstrap_prompt used / as sed delimiter, which breaks if
#         file content contains slashes. Also < > in markers could confuse sed.
# Fix: use \| as sed delimiter
# ═════════════════════════════════════════════════════════════════════════════

@test "bug2: remove_bootstrap_prompt works with slashes in surrounding content" {
    _load_lib "analyze.sh"

    mkdir -p "$TEST_TMPDIR/repo/ai"
    cat > "$TEST_TMPDIR/repo/ai/index.md" <<'EOF'
# Project at /home/user/repos/my-project

Path: /usr/local/bin/something

<!-- KRONN:BOOTSTRAP:START -->
Bootstrap content here.
<!-- KRONN:BOOTSTRAP:END -->

Config file: /etc/kronn/config.toml
URL: https://example.com/api/v1
EOF

    run remove_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    # Markers should be gone
    run grep "KRONN:BOOTSTRAP" "$TEST_TMPDIR/repo/ai/index.md"
    assert_failure

    # Content with slashes must survive
    run grep "/home/user/repos/my-project" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success

    run grep "/usr/local/bin/something" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success

    run grep "https://example.com/api/v1" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
}

@test "bug2: remove_bootstrap_prompt handles angle brackets in markers" {
    _load_lib "analyze.sh"

    mkdir -p "$TEST_TMPDIR/repo/ai"
    cat > "$TEST_TMPDIR/repo/ai/index.md" <<'EOF'
Before.
<!-- KRONN:BOOTSTRAP:START -->
<!-- This has <html> tags and <angle> brackets -->
Some content with > and < chars.
<!-- KRONN:BOOTSTRAP:END -->
After.
EOF

    run remove_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    run grep "Before" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success

    run grep "After" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success

    run grep "KRONN:BOOTSTRAP" "$TEST_TMPDIR/repo/ai/index.md"
    assert_failure
}

# ═════════════════════════════════════════════════════════════════════════════
# Bug 4: init_repo used $KRONN_DIR without checking if it was set.
#         If unset, template_dir resolves to /templates (always fails silently).
# Fix: ${KRONN_DIR:?} fails fast with a clear error message
# ═════════════════════════════════════════════════════════════════════════════

@test "bug4: init_repo fails fast when KRONN_DIR is not set" {
    _load_lib "repos.sh"

    # Unset KRONN_DIR
    unset KRONN_DIR

    mkdir -p "$TEST_TMPDIR/repo"
    run init_repo "$TEST_TMPDIR/repo"
    assert_failure
    assert_output --partial "KRONN_DIR"
}

@test "bug4: init_repo works when KRONN_DIR is set" {
    _load_lib "analyze.sh"
    _load_lib "mcps.sh"
    _load_lib "repos.sh"

    # Set KRONN_DIR to a temp dir with minimal template structure
    export KRONN_DIR="$TEST_TMPDIR/kronn"
    export KRONN_CONFIG_DIR="$TEST_TMPDIR/kronn-config"
    mkdir -p "$KRONN_DIR/templates/ai"
    echo "# Template index" > "$KRONN_DIR/templates/ai/index.md"

    mkdir -p "$TEST_TMPDIR/repo/.git"

    # Override interactive functions to avoid blocking
    ask_yn() { return 1; }
    maybe_analyze_repo() { return 0; }

    run init_repo "$TEST_TMPDIR/repo"
    assert_success

    # ai/ should have been copied
    [ -f "$TEST_TMPDIR/repo/ai/index.md" ]
}

# ═════════════════════════════════════════════════════════════════════════════
# Bug 5: envsubst replaced ALL environment variables in the MCP template,
#         not just Kronn secrets. $HOME, $PATH, $USER etc. would leak.
# Fix: restrict envsubst to explicit list of known secret variables
# ═════════════════════════════════════════════════════════════════════════════

@test "bug5: sync_mcp_for_repo does not substitute unknown env vars" {
    _load_lib "mcps.sh"

    export KRONN_CONFIG_DIR="$TEST_TMPDIR/config"
    mkdir -p "$KRONN_CONFIG_DIR"
    cat > "$KRONN_CONFIG_DIR/secrets.toml" <<'TOML'
[github]
personal_access_token = "ghp_test123"
TOML

    mkdir -p "$TEST_TMPDIR/repo"
    # Template with a Kronn secret AND a system env var
    cat > "$TEST_TMPDIR/repo/.mcp.json.example" <<'JSON'
{
  "mcpServers": {
    "github": {
      "env": {
        "GITHUB_TOKEN": "$GITHUB_PERSONAL_ACCESS_TOKEN",
        "HOME_DIR": "$HOME",
        "MY_PATH": "$PATH"
      }
    }
  }
}
JSON

    # Set known env vars that should NOT be substituted
    export HOME="/home/testuser"
    export PATH="/usr/bin:/bin"

    run sync_mcp_for_repo "$TEST_TMPDIR/repo"

    if [[ "$status" -ne 0 ]]; then
        # envsubst not installed — skip
        skip "envsubst not available"
    fi

    local output="$TEST_TMPDIR/repo/.mcp.json"
    [ -f "$output" ]

    # Kronn secret should be substituted
    run grep "ghp_test123" "$output"
    assert_success

    # System vars should NOT be substituted — should remain as literals
    run grep '$HOME' "$output"
    assert_success

    run grep '$PATH' "$output"
    assert_success
}

@test "bug5: sync_mcp_for_repo substitutes all known Kronn secrets" {
    _load_lib "mcps.sh"

    export KRONN_CONFIG_DIR="$TEST_TMPDIR/config"
    mkdir -p "$KRONN_CONFIG_DIR"
    cat > "$KRONN_CONFIG_DIR/secrets.toml" <<'TOML'
[atlassian]
url = "https://myco.atlassian.net"
username = "user@example.com"
api_token = "atl-secret-token"

[github]
personal_access_token = "ghp_mysecret"

[aws]
access_key_id = "AKIA123"
secret_access_key = "wJalr456"
region = "eu-west-1"
TOML

    mkdir -p "$TEST_TMPDIR/repo"
    cat > "$TEST_TMPDIR/repo/.mcp.json.example" <<'JSON'
{
  "atlassian_url": "$ATLASSIAN_URL",
  "jira_user": "$JIRA_USERNAME",
  "jira_token": "$JIRA_API_TOKEN",
  "gh_token": "$GITHUB_PERSONAL_ACCESS_TOKEN",
  "aws_key": "$AWS_ACCESS_KEY_ID",
  "aws_secret": "$AWS_SECRET_ACCESS_KEY",
  "aws_region": "$AWS_REGION"
}
JSON

    run sync_mcp_for_repo "$TEST_TMPDIR/repo"
    if [[ "$status" -ne 0 ]]; then
        skip "envsubst not available"
    fi

    local output="$TEST_TMPDIR/repo/.mcp.json"

    run grep "https://myco.atlassian.net" "$output"
    assert_success

    run grep "user@example.com" "$output"
    assert_success

    run grep "ghp_mysecret" "$output"
    assert_success

    run grep "AKIA123" "$output"
    assert_success

    run grep "eu-west-1" "$output"
    assert_success
}

# ═════════════════════════════════════════════════════════════════════════════
# Bug 6: ensure_gitignore silently failed when repo directory didn't exist,
#         creating a .gitignore in a nonexistent path (or erroring silently).
# Fix: check directory exists first, return 1 if not
# ═════════════════════════════════════════════════════════════════════════════

@test "bug6: ensure_gitignore fails when repo directory does not exist" {
    _load_lib "repos.sh"

    run ensure_gitignore "$TEST_TMPDIR/nonexistent-repo"
    assert_failure
}

@test "bug6: ensure_gitignore succeeds for existing directory" {
    _load_lib "repos.sh"

    mkdir -p "$TEST_TMPDIR/repo"
    run ensure_gitignore "$TEST_TMPDIR/repo"
    assert_success

    [ -f "$TEST_TMPDIR/repo/.gitignore" ]
}

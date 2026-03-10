#!/usr/bin/env bats
# ─── Tests for lib/analyze.sh ────────────────────────────────────────────────

load test_helper

setup() {
    _load_lib "ui.sh"
    _load_lib "analyze.sh"
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-analyze-test-XXXXXX)"
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# ─── inject_bootstrap_prompt ─────────────────────────────────────────────────

@test "inject_bootstrap_prompt: injects into clean file" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Original content" > "$TEST_TMPDIR/repo/ai/index.md"

    run inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    run grep "KRONN:BOOTSTRAP:START" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
    run grep "KRONN:BOOTSTRAP:END" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
    run grep "Original content" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
}

@test "inject_bootstrap_prompt: idempotent — does not double-inject" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Content" > "$TEST_TMPDIR/repo/ai/index.md"

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    local size_after_first
    size_after_first=$(wc -c < "$TEST_TMPDIR/repo/ai/index.md")

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    local size_after_second
    size_after_second=$(wc -c < "$TEST_TMPDIR/repo/ai/index.md")

    [ "$size_after_first" = "$size_after_second" ]
}

@test "inject_bootstrap_prompt: fails when index.md does not exist" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    # No index.md

    run inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_failure
}

@test "inject_bootstrap_prompt: creates temp in repo dir, not /tmp" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Content" > "$TEST_TMPDIR/repo/ai/index.md"

    run inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    # No leftover temp files
    local leftover
    leftover=$(find "$TEST_TMPDIR/repo/ai" -name '.index.md.*' 2>/dev/null | wc -l)
    [ "$leftover" -eq 0 ]
}

@test "inject_bootstrap_prompt: prepends content (original at bottom)" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Original" > "$TEST_TMPDIR/repo/ai/index.md"

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"

    # Bootstrap should be at the top, original at the bottom
    local first_line
    first_line=$(head -1 "$TEST_TMPDIR/repo/ai/index.md")
    [[ "$first_line" == *"KRONN:BOOTSTRAP:START"* ]]

    local last_lines
    last_lines=$(tail -3 "$TEST_TMPDIR/repo/ai/index.md")
    [[ "$last_lines" == *"Original"* ]]
}

# ─── has_bootstrap_prompt ────────────────────────────────────────────────────

@test "has_bootstrap_prompt: returns false for clean file" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# No prompt" > "$TEST_TMPDIR/repo/ai/index.md"

    run has_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_failure
}

@test "has_bootstrap_prompt: returns true after injection" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Content" > "$TEST_TMPDIR/repo/ai/index.md"

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"

    run has_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success
}

@test "has_bootstrap_prompt: returns false when file does not exist" {
    mkdir -p "$TEST_TMPDIR/repo/ai"

    run has_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_failure
}

# ─── remove_bootstrap_prompt ────────────────────────────────────────────────

@test "remove_bootstrap_prompt: removes markers from file" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    cat > "$TEST_TMPDIR/repo/ai/index.md" <<'EOF'
<!-- KRONN:BOOTSTRAP:START -->
Bootstrap content.
<!-- KRONN:BOOTSTRAP:END -->

# Real content
EOF

    run remove_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    run grep "KRONN:BOOTSTRAP" "$TEST_TMPDIR/repo/ai/index.md"
    assert_failure
    run grep "Real content" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
}

@test "remove_bootstrap_prompt: preserves content before and after markers" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    cat > "$TEST_TMPDIR/repo/ai/index.md" <<'EOF'
# Header before

<!-- KRONN:BOOTSTRAP:START -->
Bootstrap content.
<!-- KRONN:BOOTSTRAP:END -->

# Header after
EOF

    remove_bootstrap_prompt "$TEST_TMPDIR/repo"

    run cat "$TEST_TMPDIR/repo/ai/index.md"
    assert_output --partial "Header before"
    assert_output --partial "Header after"
    refute_output --partial "Bootstrap content"
}

@test "remove_bootstrap_prompt: handles missing file gracefully" {
    run remove_bootstrap_prompt "$TEST_TMPDIR/nonexistent"
    assert_success
}

@test "remove_bootstrap_prompt: no-op when markers absent" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    echo "# Clean file" > "$TEST_TMPDIR/repo/ai/index.md"

    local before
    before=$(cat "$TEST_TMPDIR/repo/ai/index.md")

    remove_bootstrap_prompt "$TEST_TMPDIR/repo"

    local after
    after=$(cat "$TEST_TMPDIR/repo/ai/index.md")
    [ "$before" = "$after" ]
}

@test "remove_bootstrap_prompt: works with slashes in content" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    cat > "$TEST_TMPDIR/repo/ai/index.md" <<'EOF'
# Project at /home/user/repos/my-project

<!-- KRONN:BOOTSTRAP:START -->
Bootstrap.
<!-- KRONN:BOOTSTRAP:END -->

URL: https://example.com/api/v1
EOF

    run remove_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_success

    run grep "KRONN:BOOTSTRAP" "$TEST_TMPDIR/repo/ai/index.md"
    assert_failure
    run grep "/home/user/repos" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
    run grep "https://example.com" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
}

# ─── Roundtrip ───────────────────────────────────────────────────────────────

@test "inject then remove roundtrip: content is preserved" {
    mkdir -p "$TEST_TMPDIR/repo/ai"
    printf "# My project\n\nSome documentation.\n" > "$TEST_TMPDIR/repo/ai/index.md"

    inject_bootstrap_prompt "$TEST_TMPDIR/repo"
    remove_bootstrap_prompt "$TEST_TMPDIR/repo"

    run grep "My project" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
    run grep "Some documentation" "$TEST_TMPDIR/repo/ai/index.md"
    assert_success
    run has_bootstrap_prompt "$TEST_TMPDIR/repo"
    assert_failure
}

# ─── _ANALYSIS_STEPS ─────────────────────────────────────────────────────────

@test "_ANALYSIS_STEPS: has 10 steps" {
    [ "${#_ANALYSIS_STEPS[@]}" -eq 10 ]
}

@test "_ANALYSIS_STEPS: last step is REVIEW" {
    local last="${_ANALYSIS_STEPS[${#_ANALYSIS_STEPS[@]}-1]}"
    [[ "$last" == "REVIEW|"* ]]
}

@test "_ANALYSIS_STEPS: each step has file|prompt format" {
    for entry in "${_ANALYSIS_STEPS[@]}"; do
        [[ "$entry" == *"|"* ]] || { echo "Missing pipe in: $entry"; return 1; }
    done
}

# ─── BOOTSTRAP_MARKER constants ─────────────────────────────────────────────

@test "BOOTSTRAP_MARKER_START is an HTML comment" {
    [[ "$BOOTSTRAP_MARKER_START" == "<!--"*"-->" ]]
}

@test "BOOTSTRAP_MARKER_END is an HTML comment" {
    [[ "$BOOTSTRAP_MARKER_END" == "<!--"*"-->" ]]
}

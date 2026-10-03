#!/usr/bin/env bats
# KT-960 — bind-mount sources are the user's, never created by Docker as root.

load test_helper

setup() {
    FAKE_HOME="$BATS_TEST_TMPDIR/home"
    mkdir -p "$FAKE_HOME"
    SCRIPT="$PROJECT_ROOT/scripts/docker-bind-sources.sh"
    COMPOSE="$PROJECT_ROOT/docker-compose.yml"
}

@test "every \${HOME} source of the real compose file exists afterwards" {
    run "$SCRIPT" "$COMPOSE" "$FAKE_HOME"
    assert_success
    while IFS= read -r rel; do
        [ -e "$FAKE_HOME/$rel" ] || { echo "missing: $rel"; return 1; }
    done < <(sed -n 's/^[[:space:]]*-[[:space:]]*\${HOME}\/\([^:]*\):.*/\1/p' "$COMPOSE" | sort -u)
}

@test "a file source is created as a JSON file, not as a directory" {
    run "$SCRIPT" "$COMPOSE" "$FAKE_HOME"
    assert_success
    [ -f "$FAKE_HOME/.claude.json" ]
    [ "$(cat "$FAKE_HOME/.claude.json")" = "{}" ]
    [ -d "$FAKE_HOME/.config/rtk" ]
}

@test "existing paths are left untouched and nothing is reported for them" {
    mkdir -p "$FAKE_HOME/.codex"
    echo '{"keep": true}' > "$FAKE_HOME/.claude.json"
    run "$SCRIPT" "$COMPOSE" "$FAKE_HOME"
    assert_success
    refute_output --partial "~/.codex"
    refute_output --partial "~/.claude.json"
    [ "$(cat "$FAKE_HOME/.claude.json")" = '{"keep": true}' ]
    run "$SCRIPT" "$COMPOSE" "$FAKE_HOME"
    assert_output ""
}

@test "the start targets run it before docker compose up" {
    grep -q '^start: .*_ensure-bind-sources' "$PROJECT_ROOT/Makefile"
    grep -q '^start-prod: .*_ensure-bind-sources' "$PROJECT_ROOT/Makefile"
}

#!/usr/bin/env bats
# ─── Tests for lib/service-update.sh (kronn service update | rollback) ──────
# HOME, KRONN_SERVICE_HOME and KRONN_DATA_DIR live in a temp dir; the origin is
# a local bare git repository with semver tags; the build, the health probe,
# the restart and the "anything running?" check are replaced by fakes;
# launchctl is a recording fake. The real service and $HOME are never touched.

load test_helper

setup() {
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-service-update-XXXXXX)"
    export HOME="$TEST_TMPDIR/home"
    export KRONN_SERVICE_HOME="$TEST_TMPDIR/svc"
    export KRONN_DATA_DIR="$TEST_TMPDIR/data"
    export KRONN_SOURCE_ONLY=1
    export KRONN_PORT=1
    export KRONN_SERVICE_RETRY_DELAY=0
    export KRONN_SERVICE_DRAIN_INTERVAL=0
    export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t
    mkdir -p "$HOME/.config/kronn" "$TEST_TMPDIR/bin" "$KRONN_DATA_DIR"
    CALLS="$TEST_TMPDIR/calls"
    STATE="$TEST_TMPDIR/state"
    : > "$CALLS"
    mkdir -p "$STATE"

    cat > "$TEST_TMPDIR/bin/fake-launchctl" <<'EOF'
#!/usr/bin/env bash
echo "launchctl $*" >> "$CALLS"
case "$1" in
    print) [[ -f "$STATE/loaded" ]] && { echo "state = running"; exit 0; } || exit 113 ;;
    bootstrap) touch "$STATE/loaded" ;;
    bootout) rm -f "$STATE/loaded" ;;
esac
exit 0
EOF
    chmod +x "$TEST_TMPDIR/bin/fake-launchctl"
    export CALLS STATE
    export KRONN_LAUNCHCTL="$TEST_TMPDIR/bin/fake-launchctl"
    export KRONN_SERVICE_KIND=launchd

    source "$PROJECT_ROOT/kronn"
    source "$PROJECT_ROOT/tests/bats/bats-support/src/error.bash"

    # Fakes for everything that would build, probe or restart for real.
    service_build_from_source() {
        echo "build $1" >> "$CALLS"
        mkdir -p "$1/target/release" "$1/frontend/dist"
        printf '#!/bin/sh\necho backend\n' > "$1/target/release/kronn"
        chmod +x "$1/target/release/kronn"
        echo "<html></html>" > "$1/frontend/dist/index.html"
    }
    service_wait_healthy() {
        echo "health $1" >> "$CALLS"
        [[ -z "${FAIL_HEALTH_FOR:-}" || "$1" != "$FAIL_HEALTH_FOR" ]]
    }
    # A restart on which the "new backend" migrates the database first.
    service_restart() {
        echo "restart" >> "$CALLS"
        if [[ -n "${MIGRATE_ON_RESTART:-}" ]]; then
            echo "migrated" > "$(service_db_path)"
            echo "wal" > "$(service_db_path)-wal"
            echo "shm" > "$(service_db_path)-shm"
            echo "pre-migration" > "$(service_db_backup)"
        fi
    }
    service_missing_tools() { echo; }
    service_update_active_work() { return "${FAKE_ACTIVE_RC:-1}"; }
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# Origin: a work repo + bare clone, one commit and tag per version.
add_tag() { # <version>
    local v="$1" w="$TEST_TMPDIR/work"
    if [[ ! -d "$w/.git" ]]; then
        mkdir -p "$w/lib" "$w/backend"
        git init -q "$w"
        echo "[package]" > "$w/backend/Cargo.toml"
        printf '#!/usr/bin/env bash\necho cli\n' > "$w/kronn"
        echo "# lib" > "$w/lib/ui.sh"
        printf '# Changelog\n\n' > "$w/CHANGELOG.md"
    fi
    printf '%s\n' "$v" > "$w/VERSION"
    {
        printf '# Changelog\n\n## [%s] - 2026-01-01\n\n- feature of %s\n\n' "$v" "$v"
        tail -n +3 "$w/CHANGELOG.md"
    } > "$w/CHANGELOG.new"
    mv "$w/CHANGELOG.new" "$w/CHANGELOG.md"
    git -C "$w" add -A
    git -C "$w" commit -q -m "release $v"
    git -C "$w" tag "$v"
    [[ -d "$TEST_TMPDIR/origin.git" ]] || git init -q --bare "$TEST_TMPDIR/origin.git"
    git -C "$w" push -q "$TEST_TMPDIR/origin.git" HEAD:refs/heads/work --tags
}

# A service installed at <version> from the origin's current tags.
install_at() { # <version>
    local v="$1" home="$KRONN_SERVICE_HOME"
    mkdir -p "$home"
    git clone -q "$TEST_TMPDIR/origin.git" "$home/src"
    git -C "$home/src" checkout -q --detach "$v"
    service_build_from_source "$home/src"
    service_stage_release "$home/src" "$v"
    service_switch_current "$v"
    mkdir -p "$(dirname "$(service_unit_path)")"
    touch "$(service_unit_path)"
    touch "$STATE/loaded"
    : > "$CALLS"
}

# A fake staged release with no source tree behind it.
fake_release() { # <version>
    local d="$KRONN_SERVICE_HOME/releases/$1"
    mkdir -p "$d"
    printf '%s\n' "$1" > "$d/VERSION"
}

old_time() { touch -t 202001010000 "$1"; }

# ─── Update ──────────────────────────────────────────────────────────────────

@test "update: happy path builds, switches, restarts, checks health and records previous" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.2.0" ]
    [ "$(cat "$KRONN_SERVICE_HOME/previous")" = "0.1.0" ]
    grep -q "^build " "$CALLS"
    grep -q "^restart" "$CALLS"
    grep -q "^health 0.2.0" "$CALLS"
    [ -f "$KRONN_SERVICE_HOME/releases/0.2.0/VERSION" ]
    # The old release is untouched (the running CLI lives there).
    [ -f "$KRONN_SERVICE_HOME/releases/0.1.0/cli/kronn" ]
    # Changelog section of the new version only.
    [[ "$output" == *"feature of 0.2.0"* ]]
    [[ "$output" != *"feature of 0.1.0"* ]]
    # The lock is released.
    [ ! -e "$KRONN_SERVICE_HOME/update.lock" ]
}

@test "update: refuses when the service is not installed" {
    run cmd_service_update --yes
    [ "$status" -eq 1 ]
    [[ "$output" == *"not installed"* ]]
}

@test "update: already up to date is a no-op with exit 0" {
    add_tag 0.1.0
    install_at 0.1.0
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    [[ "$output" == *"Already up to date"* ]]
    ! grep -q "^build " "$CALLS"
    ! grep -q "^restart" "$CALLS"
}

@test "update --check: exit 10 when an update exists, 0 when up to date, nothing changes" {
    add_tag 0.1.0
    install_at 0.1.0
    run cmd_service_update --check
    [ "$status" -eq 0 ]
    add_tag 0.2.0
    run cmd_service_update --check
    [ "$status" -eq 10 ]
    [[ "$output" == *"0.1.0"* && "$output" == *"0.2.0"* ]]
    [ "$(service_current_version)" = "0.1.0" ]
    ! grep -q "^build " "$CALLS"
}

@test "update --to: an unknown or malformed version is refused" {
    add_tag 0.1.0
    install_at 0.1.0
    run cmd_service_update --to 9.9.9 --yes
    [ "$status" -eq 1 ]
    [[ "$output" == *"No release tag 9.9.9"* ]]
    run cmd_service_update --to latest --yes
    [ "$status" -eq 1 ]
    [ "$(service_current_version)" = "0.1.0" ]
}

@test "update --to: picks that tag rather than the latest" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    add_tag 0.3.0
    run cmd_service_update --to 0.2.0 --yes
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.2.0" ]
}

@test "update: a dirty service clone is refused before anything is built" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    echo "local edit" >> "$KRONN_SERVICE_HOME/src/VERSION"
    run cmd_service_update --yes
    [ "$status" -eq 1 ]
    [[ "$output" == *"local modifications"* ]]
    [ "$(service_current_version)" = "0.1.0" ]
    ! grep -q "^build " "$CALLS"
}

@test "update: an already staged release is reused without a rebuild" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    : > "$CALLS"
    # Go back, then update again: 0.2.0 is still staged.
    run cmd_service_rollback --yes
    [ "$status" -eq 0 ]
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    ! grep -q "^build " "$CALLS"
    [ "$(service_current_version)" = "0.2.0" ]
}

# ─── Drain ───────────────────────────────────────────────────────────────────

@test "update drain: declining after the timeout aborts and changes nothing" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    FAKE_ACTIVE_RC=0
    service_update_active_work() { echo "1 workflow run(s)"; return 0; }
    ask_yn() { return 1; }
    KRONN_SERVICE_DRAIN_TIMEOUT=0
    run cmd_service_update
    [ "$status" -eq 1 ]
    [[ "$output" == *"Aborted"* ]]
    [ "$(service_current_version)" = "0.1.0" ]
    ! grep -q "^restart" "$CALLS"
}

@test "update drain: --yes proceeds after the timeout, --force skips the wait" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    service_update_active_work() { echo "1 workflow run(s)"; return 0; }
    KRONN_SERVICE_DRAIN_TIMEOUT=0
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.2.0" ]

    add_tag 0.3.0
    KRONN_SERVICE_DRAIN_TIMEOUT=3600
    run cmd_service_update --force
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.3.0" ]
}

@test "update drain: an unreachable API warns and proceeds" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    service_update_active_work() { return 2; }
    run cmd_service_update
    [ "$status" -eq 0 ]
    [[ "$output" == *"API unreachable"* ]]
    [ "$(service_current_version)" = "0.2.0" ]
}

@test "_su_json_count counts only Running/Pending workflow runs and plain arrays" {
    run bash -c 'source "$1"; echo "{\"data\":[{\"last_run\":{\"status\":\"Running\"}},{\"last_run\":{\"status\":\"Success\"}},{\"last_run\":null},{\"last_run\":{\"status\":\"Pending\"}},{\"last_run\":{\"status\":\"WaitingApproval\"}}]}" | _su_json_count workflows' _ "$PROJECT_ROOT/lib/service-update.sh"
    [ "$output" = "2" ]
    run bash -c 'source "$1"; echo "{\"data\":[\"a\",\"b\",\"c\"]}" | _su_json_count plain' _ "$PROJECT_ROOT/lib/service-update.sh"
    [ "$output" = "3" ]
}

# ─── Automatic rollback ──────────────────────────────────────────────────────

@test "update: an unhealthy release is rolled back and the DB is untouched without a migration" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    echo "live" > "$(service_db_path)"
    echo "old-backup" > "$(service_db_backup)"
    old_time "$(service_db_backup)"
    FAIL_HEALTH_FOR=0.2.0
    run cmd_service_update --yes
    [ "$status" -eq 1 ]
    [[ "$output" == *"rolling back"* ]]
    [ "$(service_current_version)" = "0.1.0" ]
    [ "$(cat "$(service_db_path)")" = "live" ]
    [ ! -e "$(service_db_path).failed-0.2.0" ]
    grep -q "^health 0.1.0" "$CALLS"
    [ ! -f "$KRONN_SERVICE_HOME/previous" ]
    [ ! -e "$KRONN_SERVICE_HOME/update.lock" ]
}

@test "update: an unhealthy release that migrated the DB gets the backup back, failed DB and WAL kept" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    echo "live" > "$(service_db_path)"
    echo "old-backup" > "$(service_db_backup)"
    old_time "$(service_db_backup)"
    FAIL_HEALTH_FOR=0.2.0
    MIGRATE_ON_RESTART=1
    run cmd_service_update --yes
    [ "$status" -eq 1 ]
    [ "$(service_current_version)" = "0.1.0" ]
    [ "$(cat "$(service_db_path)")" = "pre-migration" ]
    [ "$(cat "$(service_db_path).failed-0.2.0")" = "migrated" ]
    [ "$(cat "$(service_db_path).failed-0.2.0-wal")" = "wal" ]
    [ "$(cat "$(service_db_path).failed-0.2.0-shm")" = "shm" ]
    # No stale side file next to the restored database.
    [ ! -e "$(service_db_path)-wal" ]
    [ ! -e "$(service_db_path)-shm" ]
    grep -q "^launchctl bootstrap" "$CALLS"
}

# ─── Manual rollback ─────────────────────────────────────────────────────────

@test "rollback: goes back to the previous release and restores a migrated DB" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    echo "live" > "$(service_db_path)"
    echo "old-backup" > "$(service_db_backup)"
    old_time "$(service_db_backup)"
    MIGRATE_ON_RESTART=1
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    [ "$(cat "$(service_db_path)")" = "migrated" ]
    run cmd_service_rollback --yes
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.1.0" ]
    [ "$(cat "$(service_db_path)")" = "pre-migration" ]
    [ "$(cat "$(service_db_path).failed-0.2.0")" = "migrated" ]
    # A second rollback goes forward again, without touching the DB.
    run cmd_service_rollback --yes
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.2.0" ]
    [ "$(cat "$(service_db_path)")" = "pre-migration" ]
}

@test "rollback: leaves the DB alone when the update did not migrate it" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    echo "live" > "$(service_db_path)"
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    echo "newer-live" > "$(service_db_path)"
    run cmd_service_rollback --yes
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.1.0" ]
    [ "$(cat "$(service_db_path)")" = "newer-live" ]
}

@test "rollback: declining the DB restore leaves the service stopped on the new release" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    echo "live" > "$(service_db_path)"
    echo "old-backup" > "$(service_db_backup)"
    old_time "$(service_db_backup)"
    MIGRATE_ON_RESTART=1
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    ask_yn() { return 1; }
    run cmd_service_rollback
    [ "$status" -eq 1 ]
    [ "$(service_current_version)" = "0.2.0" ]
    [ "$(cat "$(service_db_path)")" = "migrated" ]
}

@test "rollback: refuses without a previous release, or when it was pruned" {
    add_tag 0.1.0
    install_at 0.1.0
    run cmd_service_rollback --yes
    [ "$status" -eq 1 ]
    [[ "$output" == *"no previous release"* ]]
    echo "0.0.9" > "$KRONN_SERVICE_HOME/previous"
    run cmd_service_rollback --yes
    [ "$status" -eq 1 ]
    [[ "$output" == *"no longer staged"* ]]
}

# ─── Prune ───────────────────────────────────────────────────────────────────

@test "prune: keeps the 3 most recent by semver plus current and previous" {
    local v
    for v in 0.1.0 0.2.0 0.3.0 0.9.0 0.10.0 0.11.0; do fake_release "$v"; done
    service_switch_current 0.11.0
    echo "0.2.0" > "$KRONN_SERVICE_HOME/previous"
    _su_prune_releases
    [ -d "$KRONN_SERVICE_HOME/releases/0.11.0" ]
    [ -d "$KRONN_SERVICE_HOME/releases/0.10.0" ]
    [ -d "$KRONN_SERVICE_HOME/releases/0.9.0" ]
    [ -d "$KRONN_SERVICE_HOME/releases/0.2.0" ]
    [ ! -d "$KRONN_SERVICE_HOME/releases/0.3.0" ]
    [ ! -d "$KRONN_SERVICE_HOME/releases/0.1.0" ]
}

@test "prune: never deletes the current release even when it is old" {
    local v
    for v in 0.1.0 0.2.0 0.3.0 0.4.0 0.5.0; do fake_release "$v"; done
    service_switch_current 0.1.0
    echo "0.2.0" > "$KRONN_SERVICE_HOME/previous"
    _su_prune_releases
    [ -d "$KRONN_SERVICE_HOME/releases/0.1.0" ]
    [ -d "$KRONN_SERVICE_HOME/releases/0.2.0" ]
    [ -d "$KRONN_SERVICE_HOME/releases/0.5.0" ]
    [ -d "$KRONN_SERVICE_HOME/releases/0.4.0" ]
    [ -d "$KRONN_SERVICE_HOME/releases/0.3.0" ]
    [ "$(service_current_version)" = "0.1.0" ]
}

# ─── Lock ────────────────────────────────────────────────────────────────────

@test "lock: a stale lock (dead pid) is cleared and the update proceeds" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    sleep 0 & local dead=$!
    wait "$dead"
    mkdir -p "$KRONN_SERVICE_HOME/update.lock"
    echo "$dead" > "$KRONN_SERVICE_HOME/update.lock/pid"
    run cmd_service_update --yes
    [ "$status" -eq 0 ]
    [[ "$output" == *"stale update lock"* ]]
    [ "$(service_current_version)" = "0.2.0" ]
}

@test "lock: a live lock refuses the update and is left in place" {
    add_tag 0.1.0
    install_at 0.1.0
    add_tag 0.2.0
    sleep 30 & local live=$!
    mkdir -p "$KRONN_SERVICE_HOME/update.lock"
    echo "$live" > "$KRONN_SERVICE_HOME/update.lock/pid"
    run cmd_service_update --yes
    kill "$live" 2>/dev/null || true
    [ "$status" -eq 1 ]
    [[ "$output" == *"Another"* ]]
    [ -d "$KRONN_SERVICE_HOME/update.lock" ]
    [ "$(service_current_version)" = "0.1.0" ]
}

# ─── Surface ─────────────────────────────────────────────────────────────────

@test "service usage and completions mention update and rollback" {
    run cmd_service_usage
    [[ "$output" == *"update"* && "$output" == *"rollback"* ]]
    grep -q "update rollback" "$PROJECT_ROOT/completions/kronn.bash"
    grep -q "'rollback:" "$PROJECT_ROOT/completions/kronn.zsh"
}

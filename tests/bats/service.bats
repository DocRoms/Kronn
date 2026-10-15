#!/usr/bin/env bats
# ─── Tests for lib/service.sh (kronn service …) ─────────────────────────────
# Nothing here touches the real service manager or the real $HOME: HOME and
# KRONN_SERVICE_HOME live in a temp dir, launchctl/systemctl/journalctl are
# fakes that record their arguments, and the service kind is forced.

load test_helper

setup() {
    TEST_TMPDIR="$(mktemp -d /tmp/kronn-service-XXXXXX)"
    export HOME="$TEST_TMPDIR/home"
    export KRONN_SERVICE_HOME="$TEST_TMPDIR/svc"
    export KRONN_SOURCE_ONLY=1
    export KRONN_PORT=1                      # never probe a real local Kronn
    export KRONN_SERVICE_RETRY_DELAY=0
    mkdir -p "$HOME/.config/kronn" "$TEST_TMPDIR/bin"
    CALLS="$TEST_TMPDIR/calls"
    STATE="$TEST_TMPDIR/state"
    : > "$CALLS"
    mkdir -p "$STATE"
    UID_NOW="$(id -u)"

    # launchctl fake: `print` succeeds with "state = running" while loaded.
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
    cat > "$TEST_TMPDIR/bin/fake-systemctl" <<'EOF'
#!/usr/bin/env bash
echo "systemctl $*" >> "$CALLS"
case "$*" in
    *is-active*) [[ -f "$STATE/loaded" ]]; exit $? ;;
    *"enable --now"*|*restart*) touch "$STATE/loaded" ;;
    *" stop "*|*" stop") rm -f "$STATE/loaded" ;;
esac
exit 0
EOF
    cat > "$TEST_TMPDIR/bin/fake-journalctl" <<'EOF'
#!/usr/bin/env bash
echo "journalctl $*" >> "$CALLS"
EOF
    chmod +x "$TEST_TMPDIR"/bin/fake-*
    export CALLS STATE
    export KRONN_LAUNCHCTL="$TEST_TMPDIR/bin/fake-launchctl"
    export KRONN_SYSTEMCTL="$TEST_TMPDIR/bin/fake-systemctl"
    export KRONN_JOURNALCTL="$TEST_TMPDIR/bin/fake-journalctl"
    export KRONN_SERVICE_KIND=launchd

    source "$PROJECT_ROOT/kronn"
    # ui.sh's fail() only prints; restore the bats assertion boundary.
    source "$PROJECT_ROOT/tests/bats/bats-support/src/error.bash"
    export KRONN_CONFIG_DIR="$HOME/.config/kronn"
    # Never pkill the developer's real Kronn from a test.
    reap_native_backend() { echo "reap" >> "$CALLS"; }
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# A fake, already built Kronn source tree (a real git repo so SOURCE_SHA works).
make_fake_tree() { # <dir> [version]
    local d="$1" v="${2:-0.15.0}"
    mkdir -p "$d/backend/scripts" "$d/lib" "$d/completions" "$d/target/release" "$d/frontend/dist"
    printf '%s\n' "$v" > "$d/VERSION"
    printf '#!/usr/bin/env bash\necho cli\n' > "$d/kronn"
    echo "# lib" > "$d/lib/ui.sh"
    echo "# comp" > "$d/completions/kronn.bash"
    echo "[package]" > "$d/backend/Cargo.toml"
    echo "print('mcp')" > "$d/backend/scripts/disc-introspection-mcp.py"
    echo "print('test')" > "$d/backend/scripts/test_disc.py"
    printf '#!/bin/sh\necho backend\n' > "$d/target/release/kronn"
    chmod +x "$d/target/release/kronn" "$d/kronn"
    echo "<html></html>" > "$d/frontend/dist/index.html"
    git -C "$d" init -q
    git -C "$d" add -A
    git -C "$d" -c user.name=t -c user.email=t@t commit -q -m init
}

stage() { # <version> → stage a fresh fake release
    make_fake_tree "$TEST_TMPDIR/tree-$1" "$1"
    service_stage_release "$TEST_TMPDIR/tree-$1" "$1"
}

# ─── Identity ────────────────────────────────────────────────────────────────

@test "service_home honours KRONN_SERVICE_HOME and defaults under \$HOME/.kronn" {
    [ "$(service_home)" = "$KRONN_SERVICE_HOME" ]
    unset KRONN_SERVICE_HOME
    [ "$(service_home)" = "$HOME/.kronn/service" ]
}

@test "service_label and service_unit_path follow the contract" {
    [ "$(service_label)" = "com.kronn.service" ]
    [ "$(service_unit_path)" = "$HOME/Library/LaunchAgents/com.kronn.service.plist" ]
    KRONN_SERVICE_KIND=systemd
    [ "$(service_unit_path)" = "$HOME/.config/systemd/user/kronn.service" ]
}

@test "service_is_installed reflects the unit file" {
    run service_is_installed
    [ "$status" -ne 0 ]
    mkdir -p "$(dirname "$(service_unit_path)")"
    touch "$(service_unit_path)"
    run service_is_installed
    [ "$status" -eq 0 ]
}

@test "service_is_running asks launchd (launchctl print) and systemd (is-active)" {
    run service_is_running
    [ "$status" -ne 0 ]
    touch "$STATE/loaded"
    run service_is_running
    [ "$status" -eq 0 ]
    grep -q "launchctl print gui/$UID_NOW/com.kronn.service" "$CALLS"
    KRONN_SERVICE_KIND=systemd
    run service_is_running
    [ "$status" -eq 0 ]
    grep -q "systemctl --user is-active --quiet kronn" "$CALLS"
}

# ─── Rendering ───────────────────────────────────────────────────────────────

@test "service_render_plist is well-formed and goes through current/" {
    local out="$TEST_TMPDIR/out.plist"
    service_render_plist "$KRONN_SERVICE_HOME" > "$out"
    python3 -I -c 'import sys, plistlib; d = plistlib.load(open(sys.argv[1], "rb")); print(d["Label"])' "$out" > "$TEST_TMPDIR/label"
    [ "$(cat "$TEST_TMPDIR/label")" = "com.kronn.service" ]
    python3 -I - "$out" "$KRONN_SERVICE_HOME" "$HOME" <<'EOF'
import plistlib, sys
d = plistlib.load(open(sys.argv[1], "rb"))
home, user = sys.argv[2], sys.argv[3]
assert d["ProgramArguments"] == [home + "/current/bin/kronn"], d["ProgramArguments"]
env = d["EnvironmentVariables"]
assert env["KRONN_SERVICE"] == "1"
assert env["KRONN_UI_DIST"] == home + "/current/ui"
assert env["KRONN_DISC_INTROSPECTION_MCP"] == home + "/current/scripts/disc-introspection-mcp.py"
assert env["HOME"] == user
assert d["RunAtLoad"] is True
assert d["KeepAlive"] == {"SuccessfulExit": False}
assert d["ThrottleInterval"] == 10
assert d["ProcessType"] == "Interactive"
assert d["LimitLoadToSessionType"] == "Aqua"
assert d["WorkingDirectory"] == user
assert d["StandardOutPath"] == user + "/Library/Logs/Kronn/kronn.out.log"
assert d["StandardErrorPath"] == user + "/Library/Logs/Kronn/kronn.err.log"
EOF
    ! grep -q "releases/" "$out"
}

@test "service_render_plist escapes XML metacharacters in paths" {
    local weird="$TEST_TMPDIR/a&b <c>"
    service_render_plist "$weird" > "$TEST_TMPDIR/weird.plist"
    python3 -I -c 'import plistlib, sys; d = plistlib.load(open(sys.argv[1], "rb")); assert d["ProgramArguments"][0].endswith("a&b <c>/current/bin/kronn")' "$TEST_TMPDIR/weird.plist"
}

@test "service_render_systemd_unit has the contract keys and goes through current/" {
    run service_render_systemd_unit "$KRONN_SERVICE_HOME"
    [ "$status" -eq 0 ]
    [[ "$output" == *"[Unit]"* && "$output" == *"[Service]"* && "$output" == *"[Install]"* ]]
    [[ "$output" == *"ExecStart=\"$KRONN_SERVICE_HOME/current/bin/kronn\""* ]]
    [[ "$output" == *"Restart=on-failure"* ]]
    [[ "$output" == *"RestartSec=10"* ]]
    [[ "$output" == *"Environment=\"KRONN_SERVICE=1\""* ]]
    [[ "$output" == *"Environment=\"KRONN_UI_DIST=$KRONN_SERVICE_HOME/current/ui\""* ]]
    [[ "$output" == *"Environment=\"KRONN_DISC_INTROSPECTION_MCP=$KRONN_SERVICE_HOME/current/scripts/disc-introspection-mcp.py\""* ]]
    [[ "$output" == *"WantedBy=default.target"* ]]
    [[ "$output" != *"releases/"* ]]
}

@test "service_render_systemd_unit quotes a % in paths" {
    run service_render_systemd_unit "$TEST_TMPDIR/50%off"
    [[ "$output" == *"50%%off/current/bin/kronn"* ]]
}

@test "KRONN_DOCS_SIDECAR is set only when the release ships it" {
    run service_render_systemd_unit "$KRONN_SERVICE_HOME"
    [[ "$output" != *"KRONN_DOCS_SIDECAR"* ]]
    mkdir -p "$KRONN_SERVICE_HOME/current/bin"
    touch "$KRONN_SERVICE_HOME/current/bin/kronn-docs-sidecar"
    run service_render_systemd_unit "$KRONN_SERVICE_HOME"
    [[ "$output" == *"KRONN_DOCS_SIDECAR=$KRONN_SERVICE_HOME/current/bin/kronn-docs-sidecar"* ]]
}

@test "service_write_unit writes the plist and creates the log dir" {
    run service_write_unit
    [ "$status" -eq 0 ]
    [ -f "$HOME/Library/LaunchAgents/com.kronn.service.plist" ]
    [ -d "$HOME/Library/Logs/Kronn" ]
    KRONN_SERVICE_KIND=systemd
    run service_write_unit
    [ -f "$HOME/.config/systemd/user/kronn.service" ]
}

# ─── Staging ─────────────────────────────────────────────────────────────────

@test "service_stage_release lays out the release per the contract" {
    make_fake_tree "$TEST_TMPDIR/tree"
    run service_stage_release "$TEST_TMPDIR/tree" 0.15.0
    [ "$status" -eq 0 ]
    local r="$KRONN_SERVICE_HOME/releases/0.15.0"
    [ -x "$r/bin/kronn" ]
    [ -f "$r/ui/index.html" ]
    [ -f "$r/scripts/disc-introspection-mcp.py" ]
    [ ! -e "$r/scripts/test_disc.py" ]
    [ -x "$r/cli/kronn" ]
    [ -f "$r/cli/lib/ui.sh" ]
    [ -f "$r/cli/completions/kronn.bash" ]
    [ -f "$r/cli/VERSION" ]
    [ "$(cat "$r/VERSION")" = "0.15.0" ]
    [ "$(cat "$r/SOURCE_SHA")" = "$(git -C "$TEST_TMPDIR/tree" rev-parse HEAD)" ]
}

@test "service_stage_release refuses to overwrite a complete release" {
    stage 0.15.0
    echo changed > "$KRONN_SERVICE_HOME/releases/0.15.0/bin/kronn"
    run service_stage_release "$TEST_TMPDIR/tree-0.15.0" 0.15.0
    [ "$status" -ne 0 ]
    [ "$(cat "$KRONN_SERVICE_HOME/releases/0.15.0/bin/kronn")" = "changed" ]
}

@test "service_stage_release re-stages an incomplete release (no VERSION)" {
    make_fake_tree "$TEST_TMPDIR/tree"
    mkdir -p "$KRONN_SERVICE_HOME/releases/0.15.0/bin"
    echo partial > "$KRONN_SERVICE_HOME/releases/0.15.0/bin/leftover"
    run service_stage_release "$TEST_TMPDIR/tree" 0.15.0
    [ "$status" -eq 0 ]
    [ -f "$KRONN_SERVICE_HOME/releases/0.15.0/VERSION" ]
    [ ! -e "$KRONN_SERVICE_HOME/releases/0.15.0/bin/leftover" ]
}

@test "service_stage_release rejects unbuilt trees and path-like versions" {
    make_fake_tree "$TEST_TMPDIR/tree"
    run service_stage_release "$TEST_TMPDIR/tree" "../evil"
    [ "$status" -ne 0 ]
    rm "$TEST_TMPDIR/tree/target/release/kronn"
    run service_stage_release "$TEST_TMPDIR/tree" 0.15.0
    [ "$status" -ne 0 ]
    [ ! -e "$KRONN_SERVICE_HOME/releases/0.15.0/VERSION" ]
}

# ─── current symlink ─────────────────────────────────────────────────────────

@test "service_current_version is empty without a current release" {
    [ -z "$(service_current_version)" ]
}

@test "service_switch_current creates, repoints and is idempotent" {
    stage 0.14.0
    stage 0.15.0
    run service_switch_current 0.14.0
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.14.0" ]
    run service_switch_current 0.15.0
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.15.0" ]
    run service_switch_current 0.15.0
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.15.0" ]
    [ -L "$KRONN_SERVICE_HOME/current" ]
    # The BSD `mv -f tmp current` trap: the link must replace `current`, never
    # land inside the release it points to, and no temp link may linger.
    run bash -c "ls -A '$KRONN_SERVICE_HOME' | grep -c '^\\.current\\.tmp'"
    [ "$output" = "0" ]
    [ -z "$(find "$KRONN_SERVICE_HOME/releases" -type l)" ]
}

@test "service_switch_current refuses an unstaged release and a real current dir" {
    run service_switch_current 9.9.9
    [ "$status" -ne 0 ]
    stage 0.15.0
    mkdir -p "$KRONN_SERVICE_HOME/current"
    run service_switch_current 0.15.0
    [ "$status" -ne 0 ]
    [ -d "$KRONN_SERVICE_HOME/current" ] && [ ! -L "$KRONN_SERVICE_HOME/current" ]
}

@test "service_switch_current never exposes a missing current to a concurrent reader" {
    stage 0.14.0
    stage 0.15.0
    service_switch_current 0.14.0
    (
        for i in $(seq 1 60); do
            service_switch_current 0.15.0
            service_switch_current 0.14.0
        done
    ) 3>&- &
    local pid=$! misses=0 reads=0
    while kill -0 "$pid" 2>/dev/null; do
        reads=$((reads + 1))
        [ -f "$KRONN_SERVICE_HOME/current/VERSION" ] || misses=$((misses + 1))
    done
    wait "$pid"
    [ "$reads" -gt 0 ]
    [ "$misses" -eq 0 ]
}

# ─── Release tags ────────────────────────────────────────────────────────────

@test "service_latest_release_tag sorts numerically and ignores non-semver tags" {
    make_fake_tree "$TEST_TMPDIR/repo"
    local t
    for t in 0.9.0 0.10.0 0.15.0 v9.9.9 0.16.0-rc1 nightly 1.0 0.2.0; do
        git -C "$TEST_TMPDIR/repo" tag "$t"
    done
    run service_latest_release_tag "$TEST_TMPDIR/repo"
    [ "$status" -eq 0 ]
    [ "$output" = "0.15.0" ]
}

@test "service_latest_release_tag is empty when there is no release tag" {
    make_fake_tree "$TEST_TMPDIR/repo"
    git -C "$TEST_TMPDIR/repo" tag nightly
    run service_latest_release_tag "$TEST_TMPDIR/repo"
    [ "$status" -eq 0 ]
    [ -z "$output" ]
}

# ─── Service manager calls ───────────────────────────────────────────────────

@test "launchd: load / restart / unload use bootstrap, kickstart -k and bootout" {
    service_write_unit
    service_load
    grep -q "launchctl bootstrap gui/$UID_NOW $HOME/Library/LaunchAgents/com.kronn.service.plist" "$CALLS"
    service_restart
    grep -q "launchctl kickstart -k gui/$UID_NOW/com.kronn.service" "$CALLS"
    service_unload
    grep -q "launchctl bootout gui/$UID_NOW/com.kronn.service" "$CALLS"
    [ ! -f "$STATE/loaded" ]
}

@test "launchd: load re-bootstraps a loaded job so an edited plist applies" {
    service_write_unit
    touch "$STATE/loaded"
    service_load
    local out
    out="$(grep -n "bootout\|bootstrap" "$CALLS" | cut -d: -f2)"
    [[ "$(echo "$out" | head -1)" == *bootout* ]]
    [[ "$(echo "$out" | tail -1)" == *bootstrap* ]]
}

@test "launchd: restart on an unloaded job loads it" {
    service_write_unit
    service_restart
    grep -q "launchctl bootstrap" "$CALLS"
    ! grep -q "kickstart" "$CALLS"
}

@test "systemd: load / restart / unload use systemctl --user" {
    KRONN_SERVICE_KIND=systemd
    service_write_unit
    service_load
    grep -q "systemctl --user daemon-reload" "$CALLS"
    grep -q "systemctl --user enable --now kronn" "$CALLS"
    service_restart
    grep -q "systemctl --user restart kronn" "$CALLS"
    service_unload
    grep -q "systemctl --user stop kronn" "$CALLS"
}

@test "service_wait_healthy accepts the expected version and rejects another" {
    cat > "$TEST_TMPDIR/bin/curl" <<'EOF'
#!/usr/bin/env bash
echo '{"ok":true,"version":"0.15.0","host_os":"mac"}'
EOF
    chmod +x "$TEST_TMPDIR/bin/curl"
    PATH="$TEST_TMPDIR/bin:$PATH"
    export KRONN_SERVICE_HEALTH_INTERVAL=0.1
    run service_wait_healthy 0.15.0 2
    [ "$status" -eq 0 ]
    run service_wait_healthy 0.16.0 1
    [ "$status" -ne 0 ]
}

@test "service_wait_healthy times out when nothing answers" {
    cat > "$TEST_TMPDIR/bin/curl" <<'EOF'
#!/usr/bin/env bash
exit 7
EOF
    chmod +x "$TEST_TMPDIR/bin/curl"
    PATH="$TEST_TMPDIR/bin:$PATH"
    export KRONN_SERVICE_HEALTH_INTERVAL=0.1
    run service_wait_healthy 0.15.0 1
    [ "$status" -ne 0 ]
}

# ─── Commands ────────────────────────────────────────────────────────────────

# Fake toolchain so the prerequisite check passes, a fake build, instant health.
fake_toolchain() {
    local t
    for t in git cargo node pnpm; do
        [[ "$t" == "git" ]] && continue
        printf '#!/bin/sh\nexit 0\n' > "$TEST_TMPDIR/bin/$t"
        chmod +x "$TEST_TMPDIR/bin/$t"
    done
    PATH="$TEST_TMPDIR/bin:$PATH"
    service_build_from_source() { return 0; }
    service_wait_healthy() { echo "$1" > "$STATE/health-version"; return 0; }
}

@test "install refuses to start next to another Kronn on the port when declined" {
    fake_toolchain
    make_fake_tree "$TEST_TMPDIR/checkout" 0.15.0
    KRONN_DIR="$TEST_TMPDIR/checkout"
    _service_port_answers() { return 0; }
    ask_yn() { return 1; }
    run cmd_service_install --from-checkout
    [ "$status" -ne 0 ]
    [[ "$output" == *"Another Kronn is already running"* ]]
    ! grep -q "bootstrap" "$CALLS"
    ! grep -q "reap" "$CALLS"
}

@test "install stops a native Kronn holding the port before loading the service" {
    fake_toolchain
    make_fake_tree "$TEST_TMPDIR/checkout" 0.15.0
    KRONN_DIR="$TEST_TMPDIR/checkout"
    _service_port_answers() { ! grep -q "reap" "$CALLS"; }
    run cmd_service_install --from-checkout --yes
    [ "$status" -eq 0 ]
    grep -q "reap" "$CALLS"
    grep -q "bootstrap" "$CALLS"
}

@test "_service_version_ge compares semver with or without v" {
    _service_version_ge v24.14.0 23.6.0
    _service_version_ge 23.6.0 23.6.0
    _service_version_ge v23.10.0 23.6.0
    ! _service_version_ge v23.5.9 23.6.0
    ! _service_version_ge v18.20.5 23.6.0
}

@test "_service_frontend_min_node reads engines.node, defaults to 23.6.0" {
    printf '{\n  "engines": {\n    "node": ">=24.1.0"\n  }\n}\n' > "$TEST_TMPDIR/pkg.json"
    [ "$(_service_frontend_min_node "$TEST_TMPDIR/pkg.json")" = "24.1.0" ]
    printf '{}\n' > "$TEST_TMPDIR/empty.json"
    [ "$(_service_frontend_min_node "$TEST_TMPDIR/empty.json")" = "23.6.0" ]
}

@test "_service_pick_node_dir skips a too-old node and fails when none fits" {
    local v
    for v in 18.20.5 24.14.0; do
        mkdir -p "$TEST_TMPDIR/n$v"
        printf '#!/bin/sh\necho v%s\n' "$v" > "$TEST_TMPDIR/n$v/node"
        chmod +x "$TEST_TMPDIR/n$v/node"
    done
    export KRONN_SERVICE_NODE_CANDIDATES="$TEST_TMPDIR/n18.20.5/node
$TEST_TMPDIR/n24.14.0/node"
    [ "$(_service_pick_node_dir 23.6.0)" = "$TEST_TMPDIR/n24.14.0" ]
    export KRONN_SERVICE_NODE_CANDIDATES="$TEST_TMPDIR/n18.20.5/node"
    run _service_pick_node_dir 23.6.0
    [ "$status" -ne 0 ]
}

@test "service_missing_tools lists what is absent" {
    printf '#!/bin/sh\n' > "$TEST_TMPDIR/bin/git"
    chmod +x "$TEST_TMPDIR/bin/git"
    local saved="$PATH"
    PATH="$TEST_TMPDIR/bin"
    run service_missing_tools
    PATH="$saved"
    [ "$output" = "cargo node pnpm" ]
}

@test "install --from-checkout stages <VERSION>-dev-<sha>, switches, loads and links the CLI" {
    fake_toolchain
    make_fake_tree "$TEST_TMPDIR/checkout" 0.15.0
    KRONN_DIR="$TEST_TMPDIR/checkout"
    local sha
    sha="$(git -C "$KRONN_DIR" rev-parse --short HEAD)"

    run cmd_service_install --from-checkout --yes
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.15.0-dev-$sha" ]
    [ -x "$KRONN_SERVICE_HOME/current/bin/kronn" ]
    [ -f "$HOME/Library/LaunchAgents/com.kronn.service.plist" ]
    grep -q "launchctl bootstrap gui/$UID_NOW" "$CALLS"
    # Health is compared with the backend's CARGO_PKG_VERSION, not the dev name.
    [ "$(cat "$STATE/health-version")" = "0.15.0" ]
    [ "$(readlink "$HOME/.local/bin/kronn")" = "$KRONN_SERVICE_HOME/current/cli/kronn" ]
    [[ "$output" == *"http://localhost:1"* ]]
}

@test "install --from-checkout gives uncommitted changes their own release name" {
    fake_toolchain
    make_fake_tree "$TEST_TMPDIR/checkout" 0.15.0
    KRONN_DIR="$TEST_TMPDIR/checkout"
    echo dirty > "$KRONN_DIR/new-file"
    run cmd_service_install --from-checkout --yes
    [ "$status" -eq 0 ]
    [[ "$(service_current_version)" == 0.15.0-dev-*-dirty* ]]
}

@test "install on systemd writes the unit and enables it" {
    fake_toolchain
    KRONN_SERVICE_KIND=systemd
    make_fake_tree "$TEST_TMPDIR/checkout" 0.15.0
    KRONN_DIR="$TEST_TMPDIR/checkout"
    run cmd_service_install --from-checkout --yes
    [ "$status" -eq 0 ]
    [ -f "$HOME/.config/systemd/user/kronn.service" ]
    grep -q "systemctl --user enable --now kronn" "$CALLS"
}

@test "install fails early and names the missing prerequisites" {
    make_fake_tree "$TEST_TMPDIR/checkout"
    KRONN_DIR="$TEST_TMPDIR/checkout"
    local saved="$PATH"
    PATH="$TEST_TMPDIR/bin:/usr/bin:/bin"
    service_missing_tools() { echo "cargo pnpm"; }
    run cmd_service_install --from-checkout --yes
    PATH="$saved"
    [ "$status" -ne 0 ]
    [[ "$output" == *"cargo pnpm"* ]]
    [ ! -e "$KRONN_SERVICE_HOME/current" ]
}

@test "install without --from-checkout builds the latest tag of the service clone" {
    fake_toolchain
    make_fake_tree "$TEST_TMPDIR/upstream" 0.15.0
    local up="$TEST_TMPDIR/upstream" c
    git -C "$up" tag 0.9.0
    echo b > "$up/b"; git -C "$up" add b; git -C "$up" -c user.name=t -c user.email=t@t commit -q -m b
    git -C "$up" tag 0.10.0
    echo c > "$up/c"; git -C "$up" add c; git -C "$up" -c user.name=t -c user.email=t@t commit -q -m c
    git -C "$up" tag 0.15.0
    c="$(git -C "$up" rev-parse HEAD)"
    echo d > "$up/d"; git -C "$up" add d; git -C "$up" -c user.name=t -c user.email=t@t commit -q -m d
    # Cloned from origin of the (fake) CLI dir; the fake build output is
    # ignored by the clone, so recreate it as the build step would.
    KRONN_DIR="$TEST_TMPDIR/upstream"
    service_build_from_source() {
        local d="$1"
        mkdir -p "$d/target/release" "$d/frontend/dist"
        printf '#!/bin/sh\n' > "$d/target/release/kronn"; chmod +x "$d/target/release/kronn"
        echo "<html></html>" > "$d/frontend/dist/index.html"
    }
    git -C "$KRONN_DIR" remote add origin "$TEST_TMPDIR/upstream"
    run cmd_service_install --yes
    [ "$status" -eq 0 ]
    [ "$(service_current_version)" = "0.15.0" ]
    [ "$(git -C "$KRONN_SERVICE_HOME/src" rev-parse HEAD)" = "$c" ]
    [ "$(cat "$KRONN_SERVICE_HOME/releases/0.15.0/SOURCE_SHA")" = "$c" ]
}

@test "start loads a stopped service, and is a no-op when it already runs" {
    service_write_unit
    run cmd_service_start
    [ "$status" -eq 0 ]
    grep -q "launchctl bootstrap" "$CALLS"
    : > "$CALLS"
    run cmd_service_start
    [ "$status" -eq 0 ]
    ! grep -q "bootstrap" "$CALLS"
}

@test "stop and restart route to bootout and kickstart -k" {
    service_write_unit
    touch "$STATE/loaded"
    run cmd_service_restart
    [ "$status" -eq 0 ]
    grep -q "launchctl kickstart -k gui/$UID_NOW/com.kronn.service" "$CALLS"
    run cmd_service_stop
    [ "$status" -eq 0 ]
    grep -q "launchctl bootout gui/$UID_NOW/com.kronn.service" "$CALLS"
}

@test "start/stop/restart refuse when the service is not installed" {
    run cmd_service_start
    [ "$status" -ne 0 ]
    run cmd_service_stop
    [ "$status" -ne 0 ]
    run cmd_service_restart
    [ "$status" -ne 0 ]
    [ ! -s "$CALLS" ]
}

@test "status shows installed, running, version, URL and health" {
    stage 0.15.0
    service_switch_current 0.15.0
    service_write_unit
    touch "$STATE/loaded"
    run cmd_service_status
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installed"*"yes"* ]]
    [[ "$output" == *"Running"*"yes"* ]]
    [[ "$output" == *"0.15.0"* ]]
    [[ "$output" == *"http://localhost:1"* ]]
    [[ "$output" == *"unreachable"* ]]
}

@test "logs tail the log files on launchd and journalctl on systemd" {
    service_write_unit
    mkdir -p "$HOME/Library/Logs/Kronn"
    echo "out-line" > "$HOME/Library/Logs/Kronn/kronn.out.log"
    echo "err-line" > "$HOME/Library/Logs/Kronn/kronn.err.log"
    run cmd_service_logs --no-follow
    [ "$status" -eq 0 ]
    [[ "$output" == *"out-line"* && "$output" == *"err-line"* ]]

    KRONN_SERVICE_KIND=systemd
    service_write_unit
    run cmd_service_logs
    grep -q "journalctl --user -u kronn -f" "$CALLS"
}

@test "uninstall removes the unit, keeps the service home and never touches the data dir" {
    stage 0.15.0
    service_switch_current 0.15.0
    service_write_unit
    touch "$STATE/loaded"
    mkdir -p "$HOME/.local/bin" "$HOME/Library/Application Support/com.kronn.kronn"
    echo db > "$HOME/Library/Application Support/com.kronn.kronn/kronn.db"
    ln -s "$KRONN_SERVICE_HOME/current/cli/kronn" "$HOME/.local/bin/kronn"

    run cmd_service_uninstall --yes
    [ "$status" -eq 0 ]
    grep -q "launchctl bootout" "$CALLS"
    [ ! -e "$HOME/Library/LaunchAgents/com.kronn.service.plist" ]
    [ ! -e "$HOME/.local/bin/kronn" ]
    [ -d "$KRONN_SERVICE_HOME/releases/0.15.0" ]
    [ "$(cat "$HOME/Library/Application Support/com.kronn.kronn/kronn.db")" = "db" ]
}

@test "uninstall --purge deletes the service home but still not the data dir" {
    stage 0.15.0
    service_write_unit
    mkdir -p "$HOME/Library/Application Support/com.kronn.kronn"
    echo db > "$HOME/Library/Application Support/com.kronn.kronn/kronn.db"
    run cmd_service_uninstall --yes --purge
    [ "$status" -eq 0 ]
    [ ! -e "$KRONN_SERVICE_HOME" ]
    [ -f "$HOME/Library/Application Support/com.kronn.kronn/kronn.db" ]
}

@test "uninstall keeps an unrelated ~/.local/bin/kronn symlink" {
    service_write_unit
    mkdir -p "$HOME/.local/bin"
    ln -s "$TEST_TMPDIR/elsewhere/kronn" "$HOME/.local/bin/kronn"
    run cmd_service_uninstall --yes
    [ -L "$HOME/.local/bin/kronn" ]
}

@test "cmd_service rejects unknown subcommands and update/rollback when absent" {
    run cmd_service bogus
    [ "$status" -ne 0 ]
    # The real lib/service-update.sh is sourced now: simulate a CLI without it.
    unset -f cmd_service_update cmd_service_rollback
    run cmd_service update
    [ "$status" -ne 0 ]
    [[ "$output" == *"not available"* ]]
}

@test "cmd_service dispatches update/rollback to lib/service-update.sh when loaded" {
    cmd_service_update() { echo "update-called $*"; }
    run cmd_service update --check
    [ "$status" -eq 0 ]
    [ "$output" = "update-called --check" ]
}

# ─── Coexistence ─────────────────────────────────────────────────────────────

@test "service_offer_stop is silent and successful when no service is installed" {
    run service_offer_stop reload
    [ "$status" -eq 0 ]
    [ ! -s "$CALLS" ] || ! grep -q "bootout" "$CALLS"
}

@test "service_offer_stop unloads a running service; resume reloads it only for dev runs" {
    service_write_unit
    touch "$STATE/loaded"
    ask_yn() { return 0; }
    service_offer_stop reload
    grep -q "launchctl bootout" "$CALLS"
    [ "$KRONN_SERVICE_STOPPED_BY_DEV" = "1" ]
    : > "$CALLS"
    service_resume_after_dev
    grep -q "launchctl bootstrap" "$CALLS"
    [ "$KRONN_SERVICE_STOPPED_BY_DEV" = "0" ]
    # Second call is a no-op.
    : > "$CALLS"
    service_resume_after_dev
    [ ! -s "$CALLS" ]
}

@test "service_offer_stop without reload leaves nothing to resume" {
    service_write_unit
    touch "$STATE/loaded"
    ask_yn() { return 0; }
    service_offer_stop
    [ "$KRONN_SERVICE_STOPPED_BY_DEV" = "0" ]
    : > "$CALLS"
    service_resume_after_dev
    [ ! -s "$CALLS" ]
}

@test "service_offer_stop returns 1 and keeps the service when declined" {
    service_write_unit
    touch "$STATE/loaded"
    ask_yn() { return 1; }
    run service_offer_stop reload
    [ "$status" -eq 1 ]
    ! grep -q "bootout" "$CALLS"
    [ -f "$STATE/loaded" ]
}

@test "kronn stop stops the service and skips the Docker stack outside a checkout" {
    service_write_unit
    touch "$STATE/loaded"
    reap_native_backend() { echo reaped >> "$CALLS"; }
    KRONN_DIR="$TEST_TMPDIR/release-cli"
    mkdir -p "$KRONN_DIR"
    run cmd_stop
    [ "$status" -eq 0 ]
    grep -q "launchctl bootout" "$CALLS"
    grep -q reaped "$CALLS"
}

@test "kronn restart restarts the service when installed" {
    service_write_unit
    touch "$STATE/loaded"
    run cmd_restart
    [ "$status" -eq 0 ]
    grep -q "kickstart -k" "$CALLS"
}

@test "kronn logs routes to the service logs when installed" {
    KRONN_SERVICE_KIND=systemd
    service_write_unit
    run cmd_logs
    [ "$status" -eq 0 ]
    grep -q "journalctl --user -u kronn -f" "$CALLS"
}

@test "web and start-dev refuse to run from a service release (no Makefile)" {
    KRONN_DIR="$TEST_TMPDIR/release-cli"
    mkdir -p "$KRONN_DIR"
    run cmd_web
    [ "$status" -ne 0 ]
    [[ "$output" == *"not from a checkout"* ]]
    run cmd_start_dev
    [ "$status" -ne 0 ]
    [[ "$output" == *"not from a checkout"* ]]
}

@test "start declines gracefully when the user keeps the running service" {
    service_write_unit
    touch "$STATE/loaded"
    ask_yn() { return 1; }
    run cmd_start
    [ "$status" -eq 0 ]
    [[ "$output" == *"keeps running as a service"* ]]
}

# ─── The real script ─────────────────────────────────────────────────────────

@test "kronn service (real script): usage, unknown subcommand, status when not installed" {
    unset KRONN_SOURCE_ONLY
    run "$PROJECT_ROOT/kronn" service
    [ "$status" -eq 0 ]
    [[ "$output" == *"install"* && "$output" == *"uninstall"* ]]

    run "$PROJECT_ROOT/kronn" service bogus
    [ "$status" -ne 0 ]

    run "$PROJECT_ROOT/kronn" service status
    [ "$status" -eq 0 ]
    [[ "$output" == *"Installed"*"no"* ]]
}

@test "kronn service (real script) start/stop/uninstall drive the fake launchctl" {
    unset KRONN_SOURCE_ONLY
    service_write_unit
    run "$PROJECT_ROOT/kronn" service start
    [ "$status" -eq 0 ]
    run "$PROJECT_ROOT/kronn" service stop
    [ "$status" -eq 0 ]
    run "$PROJECT_ROOT/kronn" service uninstall --yes
    [ "$status" -eq 0 ]
    grep -q "launchctl bootstrap" "$CALLS"
    grep -q "launchctl bootout" "$CALLS"
    [ ! -e "$HOME/Library/LaunchAgents/com.kronn.service.plist" ]
}

@test "kronn help and completions mention the service command" {
    run cmd_help
    [[ "$output" == *"service"* ]]
    grep -q "service" "$PROJECT_ROOT/completions/kronn.bash"
    grep -q "service" "$PROJECT_ROOT/completions/kronn.zsh"
}

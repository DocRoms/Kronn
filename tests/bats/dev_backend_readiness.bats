#!/usr/bin/env bats

load test_helper

setup() {
    fixture="$BATS_TEST_TMPDIR/readiness"
    mkdir -p "$fixture/bin" "$fixture/backend"
    printf '#!/usr/bin/env bash\nexit 0\n' >"$fixture/bin/cargo"
    cat >"$fixture/bin/df" <<'EOF'
#!/usr/bin/env bash
printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/test 100 0 104857600 0%% /tmp\n'
EOF
    cat >"$fixture/kronn" <<'EOF'
#!/usr/bin/env bash
trap 'exit 0' TERM INT
trap 'if [[ "$KRONN_TEST_READY_CASE" == exited_zero ]]; then exit 0; else exit 42; fi' USR2
printf '%s\n' "$$" >"$KRONN_TEST_FIXTURE/backend-pid"
while true; do /bin/sleep 0.1; done
EOF
    cat >"$fixture/bin/watchexec" <<'EOF'
#!/usr/bin/env bash
kill -USR1 "$KRONN_DEV_BACKEND_SUPERVISOR_PID"
trap 'exit 0' TERM INT
while true; do /bin/sleep 0.1; done
EOF
    # Advance only readiness sleeps, never the process-lifecycle polling.
    # Forty seconds of startup therefore pass without a real forty-second test.
    cat >"$fixture/clock" <<'EOF'
ready_ticks=0
curl() {
    local attempt
    for ((attempt=0; attempt<1000; attempt++)); do
        [[ "$(cat "$KRONN_TEST_FIXTURE/backend-pid" 2>/dev/null)" == "$backend_pid" ]] && break
        /bin/sleep 0.01
    done
    [[ "$attempt" -lt 1000 ]] || return 7
    printf '%s\n' "$ready_ticks" >>"$KRONN_TEST_FIXTURE/probes"
    case "$KRONN_TEST_READY_CASE" in
        slow) (( ready_ticks >= 400 )) ;;
        timeout) return 7 ;;
        exited|exited_zero)
            kill -USR2 "$backend_pid"
            wait "$backend_pid" 2>/dev/null || true
            return 7
            ;;
        interrupted) kill -TERM "$$" ;;
    esac
}
sleep() {
    case " ${FUNCNAME[*]} " in
        *' backend_ready '*)
            ready_ticks=$(awk -v elapsed="$ready_ticks" -v interval="$1" 'BEGIN { print elapsed + interval * 10 }')
            ;;
        *) command sleep "$@" ;;
    esac
}
EOF
    cat >"$fixture/check" <<'EOF'
#!/usr/bin/env bash
supervisor=""
cleanup() {
    if [[ -n "$supervisor" ]]; then
        kill -TERM "$supervisor" 2>/dev/null || true
        wait "$supervisor" 2>/dev/null || true
    fi
}
trap cleanup EXIT
bash "$1" >"$KRONN_TEST_FIXTURE/log" 2>&1 3>&- &
supervisor=$!
finished=0
for ((attempt=0; attempt<1000; attempt++)); do
    if grep -q 'Backend hot reload complete.' "$KRONN_TEST_FIXTURE/log"; then
        kill -0 "$(cat "$KRONN_TEST_FIXTURE/backend-pid")" || exit 1
        kill -TERM "$supervisor"
        finished=1
        break
    fi
    if ! kill -0 "$supervisor" 2>/dev/null; then
        finished=1
        break
    fi
    /bin/sleep 0.01
done
if [[ "$finished" != 1 ]]; then
    cat "$KRONN_TEST_FIXTURE/log"
    echo 'fixture deadline exceeded'
    exit 1
fi
wait "$supervisor"
status=$?
supervisor=""
cat "$KRONN_TEST_FIXTURE/log"
printf 'supervisor-status=%s\n' "$status"
EOF
    chmod +x "$fixture/bin/cargo" "$fixture/bin/df" "$fixture/bin/watchexec" "$fixture/kronn"
}

run_readiness_case() {
    run env \
        PATH="$fixture/bin:$PATH" \
        BASH_ENV="$fixture/clock" \
        KRONN_TEST_FIXTURE="$fixture" \
        KRONN_TEST_READY_CASE="$1" \
        KRONN_DEV_BACKEND_DIR="$fixture/backend" \
        KRONN_DEV_BACKEND_BINARY="$fixture/kronn" \
        KRONN_DEV_BACKEND_TARGET_DIR="$fixture/target" \
        KRONN_DEV_BACKEND_FAILURE_FILE="$fixture/failure" \
        KRONN_DEV_BACKEND_HEALTH_URL="http://fixture.invalid/health" \
        KRONN_DEV_READY_ATTEMPTS="${2:-300}" \
        KRONN_DEV_READY_INTERVAL="${3:-1}" \
        bash "$fixture/check" "$PROJECT_ROOT/scripts/dev-backend-supervisor.sh"
}

@test "hot reload allows a live backend to become ready after forty seconds" {
    run_readiness_case slow
    assert_success
    assert_output --partial 'Backend hot reload complete.'
    assert_output --partial 'supervisor-status=143'
    [[ ! -e "$fixture/failure" ]]
    [[ "$(tail -1 "$fixture/probes")" -eq 400 ]]
}

@test "hot reload honors the startup readiness budget and identifies its timeout" {
    run_readiness_case timeout 3 2
    assert_success
    assert_output --partial 'Backend readiness timed out after the hot-reload swap'
    assert_output --partial 'http://fixture.invalid/health'
    assert_output --partial 'supervisor-status=1'
    [[ "$(cat "$fixture/failure")" -eq 1 ]]
    [[ "$(wc -l <"$fixture/probes" | tr -d ' ')" -eq 3 ]]
}

@test "hot reload preserves a backend exit code before readiness" {
    run_readiness_case exited
    assert_success
    assert_output --partial 'Backend exited before readiness after the hot-reload swap (exit 42)'
    assert_output --partial 'supervisor-status=42'
    [[ "$(cat "$fixture/failure")" -eq 42 ]]
    [[ "$(wc -l <"$fixture/probes" | tr -d ' ')" -eq 1 ]]
}

@test "an intentional stop during hot-reload readiness is not a backend failure" {
    run_readiness_case interrupted
    assert_success
    assert_output --partial 'supervisor-status=143'
    [[ ! -e "$fixture/failure" ]]
}

@test "a clean backend exit before readiness still fails the hot reload" {
    run_readiness_case exited_zero
    assert_success
    assert_output --partial 'Backend exited before readiness after the hot-reload swap (exit 0)'
    assert_output --partial 'supervisor-status=1'
    [[ "$(cat "$fixture/failure")" -eq 1 ]]
}

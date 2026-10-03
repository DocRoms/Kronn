#!/usr/bin/env bats
# Security policy for the Docker deployment (KT-961 family). Every agent Kronn
# starts runs inside this container, with the user's UID, and may follow a
# prompt injection: nothing mounted here may hand it the host.
#
# A failure here means a change re-opened one of these doors. Do not relax the
# test: move the capability behind an explicit opt-in (see KT-979) instead.

load test_helper

# Fail when the command succeeds. A bare `! cmd` does not: bats ignores a
# negated command's status unless it is the test's last line.
forbid() {
    if "$@"; then
        echo "forbidden, but found: $*"
        return 1
    fi
}

setup() {
    COMPOSE="$PROJECT_ROOT/docker-compose.yml"
}

# The `- source:target[:mode]` volume lines of the backend service.
volumes() {
    sed -n 's/^[[:space:]]*-[[:space:]]*\([^#]*\)/\1/p' "$COMPOSE" | grep ':' | sed 's/[[:space:]]*$//'
}

@test "no Docker socket: it lets an agent become root on the host (KT-979)" {
    forbid grep -qF "docker.sock" "$COMPOSE"
    forbid grep -qE "^[[:space:]]+group_add:" "$COMPOSE"
}

@test "no privileged mode, added capability or host namespace" {
    forbid grep -qE "^[[:space:]]+privileged:[[:space:]]*true" "$COMPOSE"
    forbid grep -qE "^[[:space:]]+cap_add:" "$COMPOSE"
    forbid grep -qE "^[[:space:]]+(pid|network|ipc):[[:space:]]*\"?host" "$COMPOSE"
    forbid grep -qE "^[[:space:]]+security_opt:" "$COMPOSE"
}

@test "the home is mounted read-only only, never writable as a whole" {
    run bash -c "sed -n 's/^[[:space:]]*-[[:space:]]*\(\\\${HOME}:[^[:space:]#]*\).*/\1/p' '$COMPOSE'"
    assert_success
    for line in $output; do
        [[ "$line" == *":ro" ]] || { echo "writable home mount: $line"; return 1; }
    done
}

@test "no host system directory is mounted" {
    for sensitive in "/:/" "/etc:" "/root" "/var/run:" "/proc" "/sys:" "/dev:"; do
        ! volumes | grep -qE "^${sensitive}" || { echo "mounts $sensitive"; return 1; }
    done
}

@test "the generated override masks Kronn's own secrets directory" {
    grep -q "docker-secret-masks.sh" "$PROJECT_ROOT/Makefile"
    grep -q "^    .config/kronn$" "$PROJECT_ROOT/scripts/docker-secret-masks.sh"
}

@test "Claude Code can save its sessions: every resumed turn needs them" {
    dockerfile="$PROJECT_ROOT/backend/Dockerfile"
    grep -q "/home/kronn/.claude/projects /home/kronn/.claude/sessions" "$dockerfile"
    grep -qE "^[[:space:]]+/home/kronn/\.claude$" "$dockerfile"
    for target in start start-prod; do
        sed -n "/^$target:/,/^\$/p" "$PROJECT_ROOT/Makefile" | grep -q "_own-claude-volumes"
    done
    grep -q "exec -T -u 0 backend chown kronn:kronn /home/kronn/.claude/projects /home/kronn/.claude/sessions" "$PROJECT_ROOT/Makefile"
}

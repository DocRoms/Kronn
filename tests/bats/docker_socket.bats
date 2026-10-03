#!/usr/bin/env bats
# KT-979 — the host's Docker socket is never given to the container by default:
# any agent in it could use the socket to become root on the host.

load test_helper

# Fail when the command succeeds. A bare `! cmd` does not: bats ignores a
# negated command's status unless it is the test's last line.
forbid() {
    if "$@"; then
        echo "forbidden, but found: $*"
        return 1
    fi
}

@test "the base compose file mounts neither the Docker socket nor its group" {
    forbid grep -qF "/var/run/docker.sock" "$PROJECT_ROOT/docker-compose.yml"
    forbid grep -q "^    group_add:" "$PROJECT_ROOT/docker-compose.yml"
}

@test "only KRONN_DOCKER_SOCKET=1 brings the socket and its group back, with a warning" {
    grep -q "KRONN_DOCKER_SOCKET" "$PROJECT_ROOT/Makefile"
    grep -q '"$$docker_socket" = "1" \]; then echo "      - /var/run/docker.sock:/var/run/docker.sock"' "$PROJECT_ROOT/Makefile"
    grep -q "agents can control Docker, and so this machine" "$PROJECT_ROOT/Makefile"
}

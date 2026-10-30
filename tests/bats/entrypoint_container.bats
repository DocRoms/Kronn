#!/usr/bin/env bats
# KT-1468 — the backend container now STARTS AS ROOT and entrypoint.sh drops to
# the host user with `setpriv`. entrypoint_host_user.bats proves the script
# logic against fixtures; this file proves the two things fixtures cannot: that
# `setpriv` really drops privileges and that the passwd/group rewrite really
# changes who the process is, in a real kernel. A future Dockerfile change that
# breaks either would otherwise break every Docker user silently.
#
# A small image stands in for the backend one (no Rust build): debian:trixie-slim
# with the Dockerfile's OWN user-creation lines (extracted, not copied), the
# same util-linux, and the real entrypoint.sh. Skipped without a Docker daemon;
# the `test-shell` CI job has one.

load test_helper

setup_file() {
    command -v docker >/dev/null 2>&1 || skip "docker is not installed"
    docker info >/dev/null 2>&1 || skip "no Docker daemon is reachable"

    local ctx="${BATS_FILE_TMPDIR}/ctx"
    mkdir -p "$ctx"
    cp "${PROJECT_ROOT}/backend/entrypoint.sh" "$ctx/entrypoint.sh"

    # The Dockerfile's user creation: from `ARG APP_UID` to the `useradd` line.
    local user_block
    user_block="$(awk '/^ARG APP_UID/ { on = 1 } on { print } on && /useradd -u/ { exit }' \
        "${PROJECT_ROOT}/backend/Dockerfile")"
    [ -n "$user_block" ] || { echo "no user creation found in backend/Dockerfile" >&2; return 1; }

    {
        echo "FROM debian:trixie-slim"
        echo "$user_block"
        # git: the entrypoint configures it; util-linux: setpriv, as in the image.
        echo "RUN apt-get update && apt-get install -y --no-install-recommends git util-linux \\"
        echo "    && rm -rf /var/lib/apt/lists/* && setpriv --version"
        echo "COPY entrypoint.sh /usr/local/bin/entrypoint.sh"
        echo "RUN chmod +x /usr/local/bin/entrypoint.sh \\"
        echo "    && mkdir -p /data/sub && touch /data/sub/file \\"
        echo "    && chown -R kronn:kronn /data /home/kronn"
        # No USER line: like the real image, the entrypoint starts as root.
        echo 'ENTRYPOINT ["entrypoint.sh"]'
    } > "$ctx/Dockerfile"

    local image="kronn-entrypoint-test:${BATS_SUITE_TMPDIR##*/}"
    docker build -q -t "$image" "$ctx" >/dev/null \
        || { echo "docker build of the entrypoint test image failed" >&2; return 1; }
    echo "$image" > "${BATS_FILE_TMPDIR}/image"
}

teardown_file() {
    [ -f "${BATS_FILE_TMPDIR}/image" ] || return 0
    docker rmi -f "$(cat "${BATS_FILE_TMPDIR}/image")" >/dev/null 2>&1 || true
}

# Run the probe in a fresh container; extra args go to `docker run` (-e ...).
# Prints, one per line: uid=… gid=… groups=… capeff=… data=<uid:gid> file=<uid:gid>
probe() {
    docker run --rm "$@" "$(cat "${BATS_FILE_TMPDIR}/image")" sh -c '
        echo "uid=$(id -u)"
        echo "gid=$(id -g)"
        echo "groups=$(id -G)"
        echo "capeff=$(awk "/^CapEff:/ { print \$2 }" /proc/self/status)"
        echo "data=$(stat -c %u:%g /data)"
        echo "file=$(stat -c %u:%g /data/sub/file)"
    '
}

@test "a host uid/gid remap ends unprivileged with the expected ids and no capability" {
    run probe -e KRONN_HOST_UID=501 -e KRONN_HOST_GID=20
    assert_success
    assert_line "uid=501"
    assert_line "gid=20"
    assert_line "capeff=0000000000000000"
    # compose's group_add is the only source of supplementary groups; none here.
    assert_line "groups=20"
}

@test "a host uid that is an image system user (7 = lp) resolves to kronn: name, passwd entry and home" {
    run docker run --rm -e KRONN_HOST_UID=7 -e KRONN_HOST_GID=7 "$(cat "${BATS_FILE_TMPDIR}/image")" sh -c '
        echo "uid=$(id -u)"
        echo "name=$(id -un)"
        echo "entry=$(getent passwd 7)"
        echo "count=$(getent passwd | awk -F: "\$3 == 7" | wc -l)"
        echo "home=$HOME"
        echo "capeff=$(awk "/^CapEff:/ { print \$2 }" /proc/self/status)"
    '
    assert_success
    assert_line "uid=7"
    assert_line "name=kronn"
    assert_line "count=1"
    assert_line --regexp '^entry=kronn:[^:]*:7:7:[^:]*:/home/kronn:'
    assert_line "home=/home/kronn"
    assert_line "capeff=0000000000000000"
}

@test "the data volume and what is in it belong to the target ids afterwards" {
    run probe -e KRONN_HOST_UID=501 -e KRONN_HOST_GID=20
    assert_success
    assert_line "data=501:20"
    assert_line "file=501:20"
}

@test "uid 0 is refused: the process never runs as root and the image user is kept" {
    run probe -e KRONN_HOST_UID=0 -e KRONN_HOST_GID=0
    assert_success
    assert_output --partial "not a non-root uid"
    assert_line "uid=1000"
    assert_line "gid=1000"
    assert_line "capeff=0000000000000000"
    assert_line "data=1000:1000"
}

@test "without KRONN_HOST_UID/GID the image user runs unprivileged" {
    run probe
    assert_success
    assert_line "uid=1000"
    assert_line "gid=1000"
    assert_line "capeff=0000000000000000"
}

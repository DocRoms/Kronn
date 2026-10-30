#!/usr/bin/env bats
# KT-1468 — the published backend image is built once for every user, so it
# cannot bake the host's UID/GID. entrypoint.sh starts as root, aligns the
# `kronn` user with KRONN_HOST_UID / KRONN_HOST_GID, fixes ownership of what
# lives in volumes, and re-executes itself as that user: agents never run as
# root and files written to bind mounts belong to the host user.
#
# These tests run the REAL script as a pretend root: `id` says uid 0, the
# passwd/group files are fixtures, `chown` and `setpriv` are recorded instead
# of executed. `find`, `awk` and `stat` are the real ones, so the ownership
# logic (what gets chown'ed, what is pruned) is what runs in the container.

load test_helper

setup() {
    ENTRYPOINT="${BATS_TEST_DIRNAME}/../../backend/entrypoint.sh"
    TEST_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/kronn-host-user-XXXXXX")"
    STUB_BIN="$TEST_TMPDIR/bin"
    CALLS="$TEST_TMPDIR/calls.log"
    KRONN_HOME="$TEST_TMPDIR/home/kronn"
    DATA="$TEST_TMPDIR/data"
    mkdir -p "$STUB_BIN" "$KRONN_HOME/.kronn/venv" "$KRONN_HOME/.ssh" "$KRONN_HOME/.npm" "$DATA"
    : > "$CALLS"

    # What the image created is owned by the image's ids; in this fixture that
    # is the user running the test, so `find -user` genuinely matches it.
    chgrp -R "$(command id -g)" "$TEST_TMPDIR/home" "$DATA"
    # The gid is read from a directory the test made, not from `id -g`: BSD
    # filesystems give a new directory its parent's group.
    owner="$(stat -c '%u:%g' "$DATA" 2>/dev/null || stat -f '%u:%g' "$DATA")"
    IMAGE_UID="${owner%%:*}"
    IMAGE_GID="${owner##*:}"
    : > "$KRONN_HOME/.kronn/venv/pyvenv.cfg"
    : > "$KRONN_HOME/.bashrc"
    # A bind mount of the user's real ~/.ssh: a different filesystem, owned by
    # the host, that the remap must never touch.
    : > "$KRONN_HOME/.ssh/id_ed25519"
    printf '36 25 0:30 / %s rw - ext4 /dev/sda1 rw\n' "$KRONN_HOME/.ssh" > "$TEST_TMPDIR/mountinfo"

    printf 'root:x:0:0:root:/root:/bin/bash\nkronn:x:%s:%s::%s:/bin/bash\n' \
        "$IMAGE_UID" "$IMAGE_GID" "$KRONN_HOME" > "$TEST_TMPDIR/passwd"
    printf 'root:x:0:\nkronn:x:%s:\n' "$IMAGE_GID" > "$TEST_TMPDIR/group"

    # A closed PATH: only what the script needs, with the privileged commands
    # replaced by recorders.
    for real in sh awk cat rm find stat mkdir ln git grep chmod mv basename dirname sed head tr; do
        target="$(command -v "$real" 2>/dev/null || true)"
        case "$target" in /*) ln -sf "$target" "$STUB_BIN/$real" ;; esac
    done
    cat > "$STUB_BIN/id" <<'STUB'
#!/bin/sh
case "$1" in
    -u) echo "${STUB_EUID:-0}" ;;
    -G) echo "${STUB_GROUPS:-0}" ;;
esac
STUB
    cat > "$STUB_BIN/chown" <<'STUB'
#!/bin/sh
echo "chown $*" >> "$CALLS"
STUB
    cat > "$STUB_BIN/setpriv" <<'STUB'
#!/bin/sh
echo "setpriv $*" >> "$CALLS"
echo "HOME=$HOME" >> "$CALLS"
STUB
    chmod +x "$STUB_BIN/id" "$STUB_BIN/chown" "$STUB_BIN/setpriv"

    export CALLS TEST_TMPDIR
    export KRONN_PASSWD_FILE="$TEST_TMPDIR/passwd"
    export KRONN_GROUP_FILE="$TEST_TMPDIR/group"
    export KRONN_MOUNTINFO="$TEST_TMPDIR/mountinfo"
    export KRONN_DATA_DIR="$DATA"
    unset KRONN_HOST_UID KRONN_HOST_GID STUB_EUID STUB_GROUPS
}

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# The entrypoint, run as the pretend root.
entrypoint() {
    run env PATH="$STUB_BIN" /bin/sh "$ENTRYPOINT" kronn --serve
}

@test "root is remapped to the host user, then dropped: the backend never runs as root" {
    export KRONN_HOST_UID=4242 KRONN_HOST_GID=4343

    entrypoint

    assert_success
    run cat "$CALLS"
    assert_line --partial "setpriv --reuid=4242 --regid=4343 --groups=4343 $ENTRYPOINT kronn --serve"
    assert_line "HOME=$KRONN_HOME"
    refute_output --partial "--reuid=0"
    refute_output --partial "--regid=0"
    # The user and its group now carry the host ids.
    grep -q "^kronn:x:4242:4343:" "$TEST_TMPDIR/passwd"
    grep -q "^kronn:x:4343:$" "$TEST_TMPDIR/group"
    run grep -c '^kronn:' "$TEST_TMPDIR/group"
    assert_output 1
}

@test "what the image created follows the new ids; the user's bind mounts are not touched" {
    export KRONN_HOST_UID=4242 KRONN_HOST_GID=4343

    entrypoint

    assert_success
    run cat "$CALLS"
    assert_line --partial "chown -h 4242:4343 "
    assert_output --partial "$KRONN_HOME/.kronn/venv/pyvenv.cfg"
    assert_output --partial "$KRONN_HOME/.bashrc"
    # ~/.ssh is a bind mount (mountinfo): pruned, so no file of the host moves.
    refute_output --partial "id_ed25519"
    refute_output --partial "$KRONN_HOME/.ssh"
}

@test "a host gid already taken by a Debian group (macOS: 20, dialout) is claimed by the kronn group" {
    # 4000 stands for macOS's gid 20, whatever gid the machine running this has.
    printf 'root:x:0:\ndialout:x:4000:members\nkronn:x:%s:\n' "$IMAGE_GID" > "$TEST_TMPDIR/group"
    export KRONN_HOST_UID=501 KRONN_HOST_GID=4000

    entrypoint

    assert_success
    grep -q "^kronn:x:4000:members$" "$TEST_TMPDIR/group"
    run grep -c '^dialout:' "$TEST_TMPDIR/group"
    assert_output 0
    run grep -c '^kronn:' "$TEST_TMPDIR/group"
    assert_output 1
    grep -q "^kronn:x:501:4000:" "$TEST_TMPDIR/passwd"
    run cat "$CALLS"
    assert_line --partial "setpriv --reuid=501 --regid=4000 --groups=4000 "
}

@test "a host uid already used by an image account (7 is lp) is taken over: kronn is the only entry at that uid" {
    printf 'root:x:0:0:root:/root:/bin/bash\nlp:x:7:7:lp:/var/spool/lpd:/usr/sbin/nologin\nkronn:x:%s:%s::%s:/bin/bash\n' \
        "$IMAGE_UID" "$IMAGE_GID" "$KRONN_HOME" > "$TEST_TMPDIR/passwd"
    export KRONN_HOST_UID=7 KRONN_HOST_GID=4343

    entrypoint

    assert_success
    assert_output --partial "uid 7 belonged to image user(s) lp"
    # Exactly one entry owns uid 7 and it is kronn, with its own home.
    run awk -F: '$3 == 7 { print $1 ":" $6 }' "$TEST_TMPDIR/passwd"
    assert_output "kronn:$KRONN_HOME"
    run grep -c '^lp:' "$TEST_TMPDIR/passwd"
    assert_output 0
    # Unrelated accounts are untouched.
    grep -q '^root:x:0:0:' "$TEST_TMPDIR/passwd"
    run cat "$CALLS"
    assert_line --partial "setpriv --reuid=7 --regid=4343 --groups=4343 "
}

@test "a host uid held by several image accounts leaves one kronn entry and no other" {
    printf 'root:x:0:0:root:/root:/bin/bash\nlp:x:7:7::/var/spool/lpd:/bin/false\nbin:x:7:2::/bin:/bin/false\nkronn:x:%s:%s::%s:/bin/bash\n' \
        "$IMAGE_UID" "$IMAGE_GID" "$KRONN_HOME" > "$TEST_TMPDIR/passwd"
    export KRONN_HOST_UID=7 KRONN_HOST_GID=7

    entrypoint

    assert_success
    run awk -F: '$3 == 7 { n++ } END { print n + 0 }' "$TEST_TMPDIR/passwd"
    assert_output 1
    grep -q "^kronn:x:7:7:" "$TEST_TMPDIR/passwd"
}

@test "a free host uid removes no account" {
    printf 'root:x:0:0:root:/root:/bin/bash\nlp:x:7:7:lp:/var/spool/lpd:/usr/sbin/nologin\nkronn:x:%s:%s::%s:/bin/bash\n' \
        "$IMAGE_UID" "$IMAGE_GID" "$KRONN_HOME" > "$TEST_TMPDIR/passwd"
    export KRONN_HOST_UID=4242 KRONN_HOST_GID=4343

    entrypoint

    assert_success
    refute_output --partial "belonged to image user"
    grep -q '^lp:x:7:7:' "$TEST_TMPDIR/passwd"
    grep -q "^kronn:x:4242:4343:" "$TEST_TMPDIR/passwd"
}

@test "agents can never become root, whatever the host ids say" {
    for bad in 0 root -1 "" abc 1.5 99999999999; do
        : > "$CALLS"
        export KRONN_HOST_UID="$bad" KRONN_HOST_GID="$bad"

        entrypoint

        assert_success
        run cat "$CALLS"
        assert_line --partial "setpriv --reuid=$IMAGE_UID --regid=$IMAGE_GID --groups=$IMAGE_GID "
        refute_output --partial "--reuid=0"
        refute_output --partial "--regid=0"
    done
    # Nothing was rewritten for an unusable id.
    grep -q "^kronn:x:$IMAGE_UID:$IMAGE_GID:" "$TEST_TMPDIR/passwd"
}

@test "a valid host id above nine digits is honoured, up to 4294967294" {
    for big in 1000000000 2147483648 4294967294; do
        : > "$CALLS"
        export KRONN_HOST_UID="$big" KRONN_HOST_GID="$big"

        entrypoint

        assert_success
        run cat "$CALLS"
        assert_line --partial "setpriv --reuid=$big --regid=$big --groups=$big "
        refute_output --partial "is not a non-root"
    done
    grep -q "^kronn:x:4294967294:4294967294:" "$TEST_TMPDIR/passwd"
}

@test "ids past the 32-bit range, the nobody sentinel and zero-only ids keep the image's ids" {
    for bad in 4294967295 4294967296 5000000000 12345678901 000 0000000000; do
        : > "$CALLS"
        export KRONN_HOST_UID="$bad" KRONN_HOST_GID="$bad"

        entrypoint

        assert_success
        run cat "$CALLS"
        assert_line --partial "setpriv --reuid=$IMAGE_UID --regid=$IMAGE_GID --groups=$IMAGE_GID "
    done
}

@test "the id bound is compared as a number: leading zeros do not push a small id out of range" {
    export KRONN_HOST_UID=0000000501 KRONN_HOST_GID=0000004343

    entrypoint

    assert_success
    run cat "$CALLS"
    assert_line --partial "setpriv --reuid=501 --regid=4343 --groups=4343 "
}

@test "a root host uid is reported, not silently honoured" {
    export KRONN_HOST_UID=0 KRONN_HOST_GID=0

    entrypoint

    assert_output --partial "KRONN_HOST_UID='0' is not a non-root uid"
    assert_output --partial "KRONN_HOST_GID='0' is not a non-root gid"
}

@test "supplementary groups from compose (group_add) are kept, root's never" {
    export KRONN_HOST_UID=4242 KRONN_HOST_GID=4343 STUB_GROUPS="0 989 4343 27"

    entrypoint

    assert_success
    run cat "$CALLS"
    assert_line --partial "--groups=4343,989,27 "
    refute_output --partial "--groups=0"
    refute_output --partial ",0 "
    refute_output --partial ",0,"
}

@test "a host user whose ids already match the image changes no file and no ownership" {
    export KRONN_HOST_UID="$IMAGE_UID" KRONN_HOST_GID="$IMAGE_GID"
    cp "$TEST_TMPDIR/passwd" "$TEST_TMPDIR/passwd.before"
    cp "$TEST_TMPDIR/group" "$TEST_TMPDIR/group.before"

    entrypoint

    assert_success
    diff "$TEST_TMPDIR/passwd" "$TEST_TMPDIR/passwd.before"
    diff "$TEST_TMPDIR/group" "$TEST_TMPDIR/group.before"
    run cat "$CALLS"
    refute_output --partial "chown"
    assert_line --partial "setpriv --reuid=$IMAGE_UID --regid=$IMAGE_GID "
}

@test "a volume owned by the image's ids is handed to the host user, and only that volume" {
    export KRONN_HOST_UID=4242 KRONN_HOST_GID=4343

    entrypoint

    assert_success
    run cat "$CALLS"
    assert_line "chown -R 4242:4343 $DATA"
    assert_line "chown -R 4242:4343 $KRONN_HOME/.npm"
    # The bind mount is never walked recursively, and a path that is not a
    # volume of the image is not either.
    refute_output --partial "chown -R 4242:4343 $KRONN_HOME/.ssh"
    refute_output --partial "chown -R 4242:4343 $KRONN_HOME "
}

@test "a volume already owned by the host user is not walked again" {
    export KRONN_HOST_UID="$IMAGE_UID" KRONN_HOST_GID="$IMAGE_GID"

    entrypoint

    assert_success
    run cat "$CALLS"
    refute_output --partial "chown -R"
}

@test "an unprivileged start skips the whole block and runs the command" {
    export STUB_EUID=1000
    printf '#!/bin/sh\necho "ran $*" >> "$CALLS"\n' > "$STUB_BIN/kronn"
    chmod +x "$STUB_BIN/kronn"
    export HOME="$TEST_TMPDIR/home-unpriv"
    mkdir -p "$HOME"

    run env PATH="$STUB_BIN" /bin/sh "$ENTRYPOINT" kronn --serve

    assert_success
    run cat "$CALLS"
    refute_output --partial "setpriv"
    refute_output --partial "chown"
    assert_line "ran --serve"
}

@test "an image without the kronn user refuses to run as root instead of continuing" {
    printf 'root:x:0:0:root:/root:/bin/bash\n' > "$TEST_TMPDIR/passwd"

    entrypoint

    assert_failure
    assert_output --partial "no 'kronn' user in this image; refusing to run as root"
    run cat "$CALLS"
    refute_output --partial "setpriv"
}

@test "a missing setpriv refuses to run as root instead of continuing" {
    rm "$STUB_BIN/setpriv"
    export KRONN_HOST_UID=4242 KRONN_HOST_GID=4343

    entrypoint

    assert_failure
    assert_output --partial "setpriv (util-linux) is missing; refusing to run as root"
}

# ── The image itself ────────────────────────────────────────────────────────

@test "the image no longer pins the runtime user: the entrypoint drops root, the Dockerfile checks the tool" {
    dockerfile="$PROJECT_ROOT/backend/Dockerfile"
    run grep -E '^USER ' "$dockerfile"
    assert_failure
    grep -q '^ENTRYPOINT \["entrypoint.sh"\]' "$dockerfile"
    grep -qE '^    util-linux \\$' "$dockerfile"
    grep -q 'setpriv --version' "$dockerfile"
    grep -q 'exec setpriv --reuid="$want_uid" --regid="$want_gid" --groups="$groups"' "$ENTRYPOINT"
}

@test "nothing in the image depends on the host that built it" {
    dockerfile="$PROJECT_ROOT/backend/Dockerfile"
    # The only host ids are build ARGs with a default; the published image
    # keeps them and the entrypoint remaps at runtime.
    grep -q '^ARG APP_UID=1000$' "$dockerfile"
    grep -q '^ARG APP_GID=1000$' "$dockerfile"
    # Both architectures are covered wherever a binary is fetched per arch.
    run grep -c 'dpkg --print-architecture' "$dockerfile"
    [ "$output" -ge 4 ]
    # Host paths come from the checkout at run time, never from COPY.
    run grep -nE '^COPY .*(/Users/|/home/[a-z]+/)' "$dockerfile"
    assert_failure
}

@test "an image whose own kronn user were root refuses to start rather than run agents as root" {
    printf 'root:x:0:0:root:/root:/bin/bash\nkronn:x:0:0::%s:/bin/bash\n' "$KRONN_HOME" > "$TEST_TMPDIR/passwd"

    entrypoint

    assert_failure
    assert_output --partial "refusing to run as root"
    run cat "$CALLS"
    refute_output --partial "setpriv"
}

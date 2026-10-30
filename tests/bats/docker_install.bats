#!/usr/bin/env bats
# KT-1468 — `make start` / `./kronn start` never compile on the user's machine.
#
# The backend crate needs ~19 GB of RAM to build, so a published image is
# pulled instead (scripts/docker-up.sh) and a source build, when it happens
# (explicit request, or a pull that failed), is refused up front on a machine
# that cannot finish it. These tests run the REAL script against a stub
# `docker`: nothing is pulled, built or started, every call is recorded.

load test_helper

GIB=$((1024 * 1024 * 1024))

setup() {
    TEST_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/kronn-docker-install-XXXXXX")"
    STUB_BIN="$TEST_TMPDIR/bin"
    CALLS="$TEST_TMPDIR/calls.log"
    mkdir -p "$STUB_BIN" "$TEST_TMPDIR/root/scripts"
    : > "$CALLS"

    # The script resolves its tree from its own location: run a copy in a
    # private tree so the developer's VERSION / .env never leak in.
    cp "$PROJECT_ROOT/scripts/docker-up.sh" "$TEST_TMPDIR/root/scripts/docker-up.sh"
    printf '1.2.3\n' > "$TEST_TMPDIR/root/VERSION"
    UP="$TEST_TMPDIR/root/scripts/docker-up.sh"

    cat > "$STUB_BIN/docker" <<'STUB'
#!/bin/sh
echo "docker $*" >> "$CALLS"
case "$1" in
    info) printf '%s\n' "${STUB_MEM_BYTES:-}"; exit 0 ;;
    image)
        # `docker image inspect <ref>`: present only for the services listed.
        for service in ${STUB_LOCAL_IMAGES:-}; do
            case "$3" in */kronn-"$service":*) exit 0 ;; esac
        done
        exit 1 ;;
    compose)
        case "$2" in
            pull)
                case " ${STUB_PULL_FAIL:-} " in
                    *" $3 "*|*" all "*) echo "Error: pull access denied for $3" >&2; exit 1 ;;
                esac ;;
            up|build) echo "env $2 KRONN_IMAGE_TAG=$KRONN_IMAGE_TAG CARGO_PROFILE=$CARGO_PROFILE" >> "$CALLS" ;;
        esac ;;
esac
exit 0
STUB
    chmod +x "$STUB_BIN/docker"

    # No swap unless a test says so; a missing file reads as "no swap".
    export KRONN_MEMINFO="$TEST_TMPDIR/meminfo-absent"
    export CALLS
    REAL_DOCKER="$(command -v docker || true)"   # before the stub shadows it
    export PATH="$STUB_BIN:$PATH"
    # Whatever the developer exported must not reach the script or the stub:
    # the script's own inputs, the stub's knobs, and the names the .env
    # interpolation tests use. One name per iteration, so two can never fuse.
    local name
    for name in $ISOLATED_VARS; do
        unset "$name"
    done
}

ISOLATED_VARS="KRONN_IMAGE_TAG KRONN_BUILD_FROM_SOURCE KRONN_FORCE_BUILD KRONN_BUILD_MIN_GB CARGO_PROFILE DOCKER_COMP
    STUB_MEM_BYTES STUB_PULL_FAIL STUB_LOCAL_IMAGES RELEASE_TAG PINNED OTHER MAJOR MINOR R"

teardown() {
    rm -rf "$TEST_TMPDIR"
}

# The Docker VM's memory, as `docker info` reports it.
docker_memory_gib() {
    export STUB_MEM_BYTES=$(( $1 * GIB ))
}

swap_gib() {
    printf 'MemTotal: 1 kB\nSwapTotal:   %s kB\nSwapFree: 0 kB\n' $(( $1 * 1024 * 1024 )) > "$KRONN_MEMINFO"
}

@test "externally exported variables never leak into the suite" {
    local name
    for name in $ISOLATED_VARS; do
        [ -z "${!name+x}" ] || {
            echo "$name leaked into the test environment" >&2
            return 1
        }
    done
    # The two names the old setup fused into one.
    [ -z "${KRONN_IMAGE_TAG+x}" ]
    [ -z "${KRONN_BUILD_FROM_SOURCE+x}" ]
}

@test "make start pulls the published images of VERSION and starts them without compiling" {
    run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "docker compose pull backend"
    assert_line "docker compose pull frontend"
    assert_line "docker compose up -d --no-build"
    assert_line "env up KRONN_IMAGE_TAG=1.2.3 CARGO_PROFILE=fast"
    refute_output --partial "compose build"
    refute_output --partial "docker info"
}

@test "the pull path never measures memory: nothing is compiled" {
    docker_memory_gib 1   # would be refused, were a build attempted

    run "$UP"

    assert_success
    run cat "$CALLS"
    refute_output --partial "docker info"
    refute_output --partial "compose build"
}

@test "KRONN_IMAGE_TAG picks another published tag" {
    KRONN_IMAGE_TAG=0.15.1 run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "env up KRONN_IMAGE_TAG=0.15.1 CARGO_PROFILE=fast"
}

@test "an image tag set in .env is honoured when the environment has none" {
    printf 'KRONN_HOST_UID=1000\nKRONN_IMAGE_TAG=0.9.0\n' > "$TEST_TMPDIR/root/.env"

    run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "env up KRONN_IMAGE_TAG=0.9.0 CARGO_PROFILE=fast"
}

@test "a quoted image tag in .env loses its quotes, as compose reads it" {
    for line in 'KRONN_IMAGE_TAG="0.15.1"' "KRONN_IMAGE_TAG='0.15.1'" 'KRONN_IMAGE_TAG = "0.15.1"  ' \
                'export KRONN_IMAGE_TAG=0.15.1' 'KRONN_IMAGE_TAG=0.15.1 # pinned' 'KRONN_IMAGE_TAG="0.15.1" # pinned'; do
        : > "$CALLS"
        printf 'KRONN_HOST_UID=1000\n%s\n' "$line" > "$TEST_TMPDIR/root/.env"

        run "$UP"

        assert_success
        run cat "$CALLS"
        assert_line "env up KRONN_IMAGE_TAG=0.15.1 CARGO_PROFILE=fast"
    done
}

@test "an image tag in a CRLF .env carries no carriage return" {
    printf 'KRONN_IMAGE_TAG=0.15.1\r\n' > "$TEST_TMPDIR/root/.env"

    run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "env up KRONN_IMAGE_TAG=0.15.1 CARGO_PROFILE=fast"
}

@test "an empty or quoted-empty image tag in .env falls back to VERSION" {
    printf 'KRONN_IMAGE_TAG=""\n' > "$TEST_TMPDIR/root/.env"

    run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "env up KRONN_IMAGE_TAG=1.2.3 CARGO_PROFILE=fast"
}

# The tag `make start` exports for a given .env, as the stub records it.
tag_exported_for_env() { # <.env content (printf format)>
    printf "$1" > "$TEST_TMPDIR/root/.env"
    : > "$CALLS"
    "$UP" >/dev/null 2>&1
    sed -n 's/^env up KRONN_IMAGE_TAG=\(.*\) CARGO_PROFILE=.*/\1/p' "$CALLS"
}

@test "an image tag interpolated from .env is exported resolved, not as the literal \${VAR}" {
    run tag_exported_for_env 'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG=${RELEASE_TAG}\n'
    assert_output "0.15.1"

    run tag_exported_for_env 'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG=$RELEASE_TAG\n'
    assert_output "0.15.1"

    run tag_exported_for_env 'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG=v${RELEASE_TAG}-x\n'
    assert_output "v0.15.1-x"

    run tag_exported_for_env 'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG="${RELEASE_TAG}"\n'
    assert_output "0.15.1"

    run tag_exported_for_env 'RELEASE_TAG=0.15.1\r\nKRONN_IMAGE_TAG=${RELEASE_TAG}\r\n'
    assert_output "0.15.1"

    run tag_exported_for_env 'MAJOR=0\nMINOR=15\nKRONN_IMAGE_TAG=${MAJOR}.${MINOR}.1\n'
    assert_output "0.15.1"
}

@test "compose interpolation defaults and alternatives work in the .env image tag" {
    run tag_exported_for_env 'KRONN_IMAGE_TAG=${PINNED:-0.9.0}\n'
    assert_output "0.9.0"

    run tag_exported_for_env 'KRONN_IMAGE_TAG=${PINNED-${OTHER:-0.8.0}}\n'
    assert_output "0.8.0"

    run tag_exported_for_env 'PINNED=0.7.0\nKRONN_IMAGE_TAG=${PINNED:-0.9.0}\n'
    assert_output "0.7.0"

    run tag_exported_for_env 'PINNED=x\nKRONN_IMAGE_TAG=${PINNED:+0.6.0}\n'
    assert_output "0.6.0"

    # `-` keeps a variable that is set but empty; `:-` replaces it.
    run tag_exported_for_env 'PINNED=\nKRONN_IMAGE_TAG=${PINNED:-0.5.0}\n'
    assert_output "0.5.0"

    # $$ is a literal dollar, and single quotes switch interpolation off.
    run tag_exported_for_env 'KRONN_IMAGE_TAG=a$$b\n'
    assert_output 'a$b'
    run tag_exported_for_env "RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG='\${RELEASE_TAG}'\n"
    assert_output '${RELEASE_TAG}'
}

@test "the process environment overrides .env when a tag is interpolated, as in compose" {
    printf 'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG=${RELEASE_TAG}\n' > "$TEST_TMPDIR/root/.env"

    RELEASE_TAG=9.9.9 run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "env up KRONN_IMAGE_TAG=9.9.9 CARGO_PROFILE=fast"
}

@test "an image tag set but empty in the environment wins over .env and means VERSION, as in compose" {
    printf 'KRONN_IMAGE_TAG=0.15.0\n' > "$TEST_TMPDIR/root/.env"

    KRONN_IMAGE_TAG="" run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "env up KRONN_IMAGE_TAG=1.2.3 CARGO_PROFILE=fast"
    refute_output --partial "0.15.0"

    # Unset is different: .env applies.
    : > "$CALLS"
    run "$UP"
    run cat "$CALLS"
    assert_line "env up KRONN_IMAGE_TAG=0.15.0 CARGO_PROFILE=fast"
}

@test "an interpolated variable set but empty in the environment beats .env, as in compose" {
    printf 'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG=${RELEASE_TAG:-0.3.0}\n' > "$TEST_TMPDIR/root/.env"

    RELEASE_TAG="" run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "env up KRONN_IMAGE_TAG=0.3.0 CARGO_PROFILE=fast"
}

@test "a variable defined below the tag, or nowhere, interpolates to nothing and falls back to VERSION" {
    run tag_exported_for_env 'KRONN_IMAGE_TAG=${RELEASE_TAG}\nRELEASE_TAG=0.15.1\n'
    assert_output "1.2.3"

    run tag_exported_for_env 'KRONN_IMAGE_TAG=${NOPE}\n'
    assert_output "1.2.3"
}

@test "the script's own variable names do not leak into .env interpolation" {
    # ROOT, TAG, COMPOSE... are shell variables of docker-up.sh, not environment.
    run tag_exported_for_env 'KRONN_IMAGE_TAG=${TAG:-0.4.0}${ROOT}${COMPOSE}\n'
    assert_output "0.4.0"
}

@test "the image tag read from .env is the one real docker compose resolves" {
    if [ -z "$REAL_DOCKER" ] || ! "$REAL_DOCKER" compose version >/dev/null 2>&1; then
        skip "docker compose is not installed"
    fi
    printf 'services:\n  a:\n    image: x/y:${KRONN_IMAGE_TAG:-0.0.0}\n' > "$TEST_TMPDIR/compose.yml"
    local env_content compose_tag script_tag
    for env_content in \
        'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG=${RELEASE_TAG}\n' \
        'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG=v$RELEASE_TAG\n' \
        'RELEASE_TAG=0.15.1\nKRONN_IMAGE_TAG="${RELEASE_TAG}-x"\n' \
        'KRONN_IMAGE_TAG=${PINNED:-1.0.0}\n' \
        'KRONN_IMAGE_TAG=${PINNED-1.0.0}\n' \
        'PINNED=\nKRONN_IMAGE_TAG=${PINNED:-1.0.0}\n' \
        'PINNED=1\nKRONN_IMAGE_TAG=${PINNED:+2.0}\n' \
        'KRONN_IMAGE_TAG=a$$b\n' \
        'R=x\nKRONN_IMAGE_TAG=${R}${R}.$R\n' \
        'KRONN_IMAGE_TAG=${PINNED:-${OTHER:-d}}\n'; do
        printf "$env_content" > "$TEST_TMPDIR/.env"
        compose_tag="$(cd "$TEST_TMPDIR" && "$REAL_DOCKER" compose -f compose.yml --env-file .env config --images 2>/dev/null | sed 's|^x/y:||')"
        script_tag="$(tag_exported_for_env "$env_content")"
        [ "$script_tag" = "$compose_tag" ] || {
            echo "compose resolves '$compose_tag', docker-up.sh exports '$script_tag' for: $env_content" >&2
            return 1
        }
    done
}

@test "an environment tag set but empty resolves the same tag in docker-up.sh and in real docker compose" {
    if [ -z "$REAL_DOCKER" ] || ! "$REAL_DOCKER" compose version >/dev/null 2>&1; then
        skip "docker compose is not installed"
    fi
    printf 'services:\n  a:\n    image: x/y:${KRONN_IMAGE_TAG:-1.2.3}\n' > "$TEST_TMPDIR/compose.yml"
    printf 'KRONN_IMAGE_TAG=0.15.0\n' > "$TEST_TMPDIR/.env"
    cp "$TEST_TMPDIR/.env" "$TEST_TMPDIR/root/.env"
    local compose_tag script_tag
    compose_tag="$(cd "$TEST_TMPDIR" && KRONN_IMAGE_TAG="" "$REAL_DOCKER" compose -f compose.yml --env-file .env config --images 2>/dev/null | sed 's|^x/y:||')"
    : > "$CALLS"
    KRONN_IMAGE_TAG="" "$UP" >/dev/null 2>&1
    script_tag="$(sed -n 's/^env up KRONN_IMAGE_TAG=\(.*\) CARGO_PROFILE=.*/\1/p' "$CALLS")"
    echo "compose '$compose_tag', docker-up.sh '$script_tag'" >&2
    [ "$compose_tag" = "1.2.3" ]
    [ "$script_tag" = "$compose_tag" ]
}

@test "an offline restart starts the images already on this machine instead of compiling" {
    docker_memory_gib 1   # a build would be refused: it must not even be considered
    export STUB_PULL_FAIL="all" STUB_LOCAL_IMAGES="backend frontend"

    run "$UP"

    assert_success
    assert_output --partial "starting the copy already on this machine"
    run cat "$CALLS"
    assert_line "docker image inspect ghcr.io/docroms/kronn-backend:1.2.3"
    assert_line "docker image inspect ghcr.io/docroms/kronn-frontend:1.2.3"
    assert_line "docker compose up -d --no-build"
    refute_output --partial "compose build"
    refute_output --partial "docker info"
}

@test "only the service with no local copy is built after a failed pull" {
    docker_memory_gib 32
    export STUB_PULL_FAIL="all" STUB_LOCAL_IMAGES="backend"

    run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "docker compose build frontend"
    refute_line "docker compose build backend"
    refute_line "docker compose build backend frontend"
    refute_output --partial "docker info"
}

@test "the local copy is looked up under the tag that would have been pulled" {
    export STUB_PULL_FAIL="all" STUB_LOCAL_IMAGES="backend frontend"

    KRONN_IMAGE_TAG=0.15.1 run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "docker image inspect ghcr.io/docroms/kronn-backend:0.15.1"
}

@test "a pull that succeeds never looks for a local copy" {
    run "$UP"

    assert_success
    run cat "$CALLS"
    refute_output --partial "image inspect"
}

@test "an explicit source build ignores local images: it rebuilds" {
    docker_memory_gib 32
    export STUB_LOCAL_IMAGES="backend frontend"

    KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "docker compose build backend frontend"
}

@test "a backend pull failure falls back to a source build of the backend only" {
    docker_memory_gib 32
    export STUB_PULL_FAIL="backend"

    run "$UP"

    assert_success
    assert_output --partial "Could not pull backend:1.2.3"
    run cat "$CALLS"
    assert_line "docker compose pull frontend"
    assert_line "docker compose build backend"
    refute_line "docker compose build backend frontend"
    assert_line "docker compose up -d --no-build"
}

@test "a frontend-only pull failure builds the frontend without a memory check" {
    export STUB_PULL_FAIL="frontend"
    docker_memory_gib 1

    run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "docker compose build frontend"
    refute_output --partial "docker info"
    refute_output --partial "compose build backend"
}

@test "an offline machine (every pull fails) builds both services from source" {
    docker_memory_gib 32
    export STUB_PULL_FAIL="all"

    run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "docker compose build backend frontend"
    assert_line "docker compose up -d --no-build"
}

@test "KRONN_BUILD_FROM_SOURCE=1 skips the pull and builds from source" {
    docker_memory_gib 32

    KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_success
    run cat "$CALLS"
    refute_output --partial "compose pull"
    assert_line "docker compose build backend frontend"
    assert_line "env build KRONN_IMAGE_TAG=1.2.3 CARGO_PROFILE=fast"
    assert_line "docker compose up -d --no-build"
}

@test "the build profile can still be chosen" {
    docker_memory_gib 32

    KRONN_BUILD_FROM_SOURCE=1 CARGO_PROFILE=release run "$UP"

    assert_success
    run cat "$CALLS"
    assert_line "env build KRONN_IMAGE_TAG=1.2.3 CARGO_PROFILE=release"
}

@test "the guard refuses a 5.8 GB WSL machine with 4 GB of swap BEFORE compiling, and names the alternatives" {
    export STUB_MEM_BYTES=$(( 58 * GIB / 10 ))
    swap_gib 4

    KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_failure
    assert_output --partial "Not enough memory to build Kronn from source"
    assert_output --partial "5.8 GiB of RAM + 4.0 GiB of swap = 9.8 GiB"
    assert_output --partial "~19 GiB"
    assert_output --partial "10.4 GiB"
    # The alternatives: published image, desktop installer, bigger machine.
    assert_output --partial "docker pull ghcr.io/docroms/kronn-backend:1.2.3"
    assert_output --partial "https://github.com/DocRoms/Kronn/releases/latest"
    assert_output --partial "Windows .exe"
    assert_output --partial "./kronn start-dev"
    assert_output --partial "KRONN_FORCE_BUILD=1"
    # Nothing was compiled or started.
    run cat "$CALLS"
    refute_output --partial "compose build"
    refute_output --partial "compose up"
}

@test "the guard also stops the fallback build after a failed pull" {
    docker_memory_gib 6
    export STUB_PULL_FAIL="backend"

    run "$UP"

    assert_failure
    assert_output --partial "published image 1.2.3 could not be pulled"
    assert_output --partial "Not enough memory"
    run cat "$CALLS"
    refute_output --partial "compose build"
    refute_output --partial "compose up"
}

@test "swap counts toward the memory a build can use" {
    docker_memory_gib 8
    swap_gib 12   # 20 GiB: the need of the default build

    KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_success
    refute_output --partial "Not enough memory"
    run cat "$CALLS"
    assert_line "docker compose build backend frontend"
}

@test "KRONN_FORCE_BUILD=1 overrides the guard" {
    docker_memory_gib 4

    KRONN_FORCE_BUILD=1 KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_success
    assert_output --partial "Not enough memory to build Kronn from source"
    assert_output --partial "KRONN_FORCE_BUILD=1: building anyway"
    run cat "$CALLS"
    assert_line "docker compose build backend frontend"
    assert_line "docker compose up -d --no-build"
}

@test "a 16 GiB machine with no swap is refused: the default build peaks at ~19 GiB" {
    docker_memory_gib 16

    KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_failure
    assert_output --partial "Not enough memory to build Kronn from source"
    assert_output --partial "16.0 GiB of RAM + 0.0 GiB of swap = 16.0 GiB"
    assert_output --partial "peaks at ~19 GiB and needs ~20 GiB"
    assert_output --partial "10.4 GiB, 12 min"
    assert_output --partial "KRONN_FORCE_BUILD=1"
    run cat "$CALLS"
    refute_output --partial "compose build"
    refute_output --partial "compose up"
}

@test "a 16 GiB machine can still force the build" {
    docker_memory_gib 16

    KRONN_FORCE_BUILD=1 KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_success
    assert_output --partial "KRONN_FORCE_BUILD=1: building anyway"
    run cat "$CALLS"
    assert_line "docker compose build backend frontend"
}

@test "the guard accepts exactly the measured need, and refuses a GiB below it" {
    docker_memory_gib 20
    KRONN_BUILD_FROM_SOURCE=1 run "$UP"
    assert_success

    docker_memory_gib 19
    KRONN_BUILD_FROM_SOURCE=1 run "$UP"
    assert_failure
}

@test "a machine with enough memory builds without a warning" {
    docker_memory_gib 32

    KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_success
    refute_output --partial "Not enough memory"
}

@test "the thresholds can be tuned" {
    docker_memory_gib 8

    KRONN_BUILD_MIN_GB=6 KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_success
    refute_output --partial "Not enough memory"
}

@test "memory Docker cannot report does not block the build" {
    export STUB_MEM_BYTES=""

    KRONN_BUILD_FROM_SOURCE=1 run "$UP"

    assert_success
    assert_output --partial "Could not measure the memory Docker can use"
    run cat "$CALLS"
    assert_line "docker compose build backend frontend"
}

@test "make start goes through the script and make start-build forces the source build" {
    grep -q '^	@./scripts/docker-up.sh$' "$PROJECT_ROOT/Makefile"
    grep -q '^start-build: export KRONN_BUILD_FROM_SOURCE := 1$' "$PROJECT_ROOT/Makefile"
    grep -q '^start-build: start$' "$PROJECT_ROOT/Makefile"
    # No recipe compiles on its own any more: every build goes through the guard.
    run grep -nE 'up -d --build|CARGO_PROFILE=fast .*up' "$PROJECT_ROOT/Makefile"
    assert_failure
}

@test "the compose services keep a build section next to the published image of VERSION" {
    version="$(tr -d '[:space:]' < "$PROJECT_ROOT/VERSION")"
    for service in backend frontend; do
        grep -qF "image: ghcr.io/docroms/kronn-$service:\${KRONN_IMAGE_TAG:-$version}" "$PROJECT_ROOT/docker-compose.yml"
    done
    [ "$(grep -c '^    build:$' "$PROJECT_ROOT/docker-compose.yml")" -eq 2 ]
}

@test "./kronn start reaches the script through make start and asks for the kronn user in exec" {
    grep -q 'make start' "$PROJECT_ROOT/kronn"
    # The container now starts as root: an unqualified exec would run as root.
    run grep -nE 'compose exec (-T )?backend|DOCKER_COMP\) exec backend' "$PROJECT_ROOT/kronn" "$PROJECT_ROOT/Makefile"
    assert_failure
    run grep -c 'exec -T -u kronn backend' "$PROJECT_ROOT/kronn"
    assert_output 2
    grep -q 'DOCKER_COMP) exec -u kronn backend' "$PROJECT_ROOT/Makefile"
}

@test "no e2e probe or doc runs a bare docker exec into the backend container" {
    # The container starts as root: a bare exec reads /root and leaves root-owned
    # files (config.toml in /data would stop being writable by the backend).
    local files=(
        "$PROJECT_ROOT/frontend/e2e/specs/per-agent-mcp-introspection.spec.ts"
        "$PROJECT_ROOT/docs/operations/debug-operations.md"
    )
    run grep -hE 'docker exec .*kronn-backend' "${files[@]}"
    assert_success
    run bash -c 'grep -hE "docker exec .*kronn-backend" "$@" | grep -vc -e "-u kronn"' _ "${files[@]}"
    assert_output 0
}

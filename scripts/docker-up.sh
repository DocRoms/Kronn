#!/usr/bin/env bash
# Start the Docker stack WITHOUT compiling on the user's machine (KT-1468).
#
# The backend is one 715 000-line crate: rustc peaks at ~19 GB on the `fast`
# profile, so a machine with less memory has its compiler SIGKILLed after
# minutes of work. Every release publishes the backend and frontend images to
# GHCR (.github/workflows/docker-images.yml); this script
#
#   1. pulls the image of the checked-out version, and starts it with
#      `--no-build`;
#   2. when a service cannot be pulled (offline, a VERSION not published yet, a
#      package not public yet) but that exact image is already on this machine,
#      starts it as it is: an offline restart never recompiles;
#   3. builds from source only when asked (KRONN_BUILD_FROM_SOURCE=1, i.e.
#      `make start-build`) or when an image is neither pullable nor local;
#   4. before ANY backend compile, measures the memory Docker can use and stops
#      at once, naming the alternatives, instead of ending in an OOM kill.
#
# Environment:
#   KRONN_IMAGE_TAG          image tag to pull (default: .env, then VERSION); set
#                            but empty, it means VERSION and .env is not read,
#                            as in compose
#   KRONN_BUILD_FROM_SOURCE  1 = skip the pull and build from source
#   KRONN_FORCE_BUILD        1 = build even when the memory guard refuses
#   KRONN_BUILD_MIN_GB       refuse below this much memory+swap (default 20,
#                            the measured need of the default build)
#   KRONN_MEMINFO            file read for swap (default /proc/meminfo; tests)
#   DOCKER_COMP              compose command (default `docker compose`)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

if [ -t 2 ]; then
    RED=$'\033[0;31m'; YELLOW=$'\033[0;33m'; CYAN=$'\033[0;36m'; GREEN=$'\033[0;32m'; RESET=$'\033[0m'
else
    RED=''; YELLOW=''; CYAN=''; GREEN=''; RESET=''
fi

# Measured on the `kronn` crate alone (dependencies already built), KT-1468.
FAST_PEAK_GB=19      # profile `fast`: 2m50, codegen-units 16 in parallel
SINGLE_THREAD_GB=10.4  # one codegen thread (-j1): 12 min
# What the default build needs: its 19 GB peak plus headroom for the rest of
# the build. Below this the compiler is killed, so the build is refused.
BUILD_NEED_GB=20
# Where compose pulls from (docker-compose.yml `image:`).
IMAGE_PREFIX="ghcr.io/docroms"
RELEASES_URL="https://github.com/DocRoms/Kronn/releases/latest"

COMPOSE="${DOCKER_COMP:-docker compose}"

info() { printf '%s\n' "$*" >&2; }

# Value of a variable the way compose looks it up while reading .env: the
# process environment first (it overrides the file), then the variables the file
# defined ABOVE the line being read. Sets LOOKUP_SET (1/0) and LOOKUP_VAL.
# `printenv`, not an indirect expansion of the name: this script's own shell
# variables (ROOT, TAG...) must never answer for a variable named in .env.
dotenv_lookup() { # <name>
    local ref="KRONN_DOTENV__$1"
    LOOKUP_SET=1
    if LOOKUP_VAL="$(printenv "$1" 2>/dev/null)"; then
        return 0
    fi
    if [ -n "${!ref+x}" ]; then
        LOOKUP_VAL="${!ref}"
        return 0
    fi
    LOOKUP_SET=0
    LOOKUP_VAL=''
}

# Compose interpolation of one value: $$ (a literal $), $NAME, ${NAME},
# ${NAME:-d} ${NAME-d} ${NAME:+a} ${NAME+a} ${NAME:?e} ${NAME?e}, the default
# or alternative being interpolated itself. A variable that is not set is empty.
dotenv_expand() { # <value>  -> stdout
    local s="$1" out="" rest inner name tail word depth i ch def
    while [ -n "$s" ]; do
        case "$s" in
            '$$'*)
                out+='$'
                s="${s#??}"
                ;;
            '${'*)
                rest="${s#??}"
                depth=1
                i=0
                while [ "$i" -lt "${#rest}" ]; do
                    ch="${rest:$i:1}"
                    if [ "$ch" = '{' ]; then
                        depth=$((depth + 1))
                    elif [ "$ch" = '}' ]; then
                        depth=$((depth - 1))
                        [ "$depth" -eq 0 ] && break
                    fi
                    i=$((i + 1))
                done
                if [ "$depth" -ne 0 ]; then # no closing brace: compose rejects it; keep it literal
                    out+="$s"
                    break
                fi
                inner="${rest:0:$i}"
                s="${rest:$((i + 1))}"
                name="${inner%%[!A-Za-z0-9_]*}"
                tail="${inner#"$name"}"
                dotenv_lookup "$name"
                word="$LOOKUP_VAL"
                case "$tail" in
                    ':-'*) def="${tail#:-}"; if [ -n "$word" ]; then out+="$word"; else out+="$(dotenv_expand "$def")"; fi ;;
                    '-'*)  def="${tail#-}";  if [ "$LOOKUP_SET" = 1 ]; then out+="$word"; else out+="$(dotenv_expand "$def")"; fi ;;
                    ':+'*) def="${tail#:+}"; if [ -n "$word" ]; then out+="$(dotenv_expand "$def")"; fi ;;
                    '+'*)  def="${tail#+}";  if [ "$LOOKUP_SET" = 1 ]; then out+="$(dotenv_expand "$def")"; fi ;;
                    *)     out+="$word" ;; # plain ${NAME}; ${NAME:?e} / ${NAME?e}: compose itself stops on an empty one
                esac
                ;;
            '$'[A-Za-z_]*)
                rest="${s#?}"
                name="${rest%%[!A-Za-z0-9_]*}"
                s="${rest#"$name"}"
                dotenv_lookup "$name"
                out+="$LOOKUP_VAL"
                ;;
            *)
                out+="${s:0:1}"
                s="${s:1}"
                ;;
        esac
    done
    printf '%s' "$out"
}

# KRONN_IMAGE_TAG as compose reads it from .env, line by line: an optional
# `export`, spaces around the `=` and the value, one pair of matching single or
# double quotes around the value (stripped), an unquoted value ending at ` #`,
# CRLF line endings (a .env edited on Windows) that are not part of the value,
# the last assignment wins, and $VAR / ${VAR:-x} references are interpolated
# (never inside single quotes): `KRONN_IMAGE_TAG=${RELEASE_TAG}` is the tag
# RELEASE_TAG holds, not that literal text, which is no image reference.
env_file_tag() {
    local line name value
    local re='^[[:space:]]*(export[[:space:]]+)?([A-Za-z_][A-Za-z0-9_]*)[[:space:]]*=(.*)$'
    [ -f .env ] || return 0
    while IFS= read -r line || [ -n "$line" ]; do
        line="${line%$'\r'}"
        [[ "$line" =~ $re ]] || continue
        name="${BASH_REMATCH[2]}"
        value="${BASH_REMATCH[3]}"
        value="${value#"${value%%[![:space:]]*}"}"
        case "$value" in
            \"*\"*) value="${value#\"}"; value="$(dotenv_expand "${value%%\"*}")" ;;
            \'*\'*) value="${value#\'}"; value="${value%%\'*}" ;;
            *)
                value="${value%%[[:space:]]#*}"
                value="${value%"${value##*[![:space:]]}"}"
                value="$(dotenv_expand "$value")"
                ;;
        esac
        printf -v "KRONN_DOTENV__$name" '%s' "$value"
    done < .env
    printf '%s' "${KRONN_DOTENV__KRONN_IMAGE_TAG:-}"
}

image_tag() {
    local tag
    if [ -n "${KRONN_IMAGE_TAG+x}" ]; then
        # Set in the environment, even to nothing: it overrides .env like it does
        # for compose, and an empty one falls to the default (VERSION), not to the file.
        tag="$KRONN_IMAGE_TAG"
    else
        tag="$(env_file_tag)"
    fi
    if [ -n "$tag" ]; then
        printf '%s' "$tag"
    else
        tr -d '[:space:]' < VERSION
    fi
}

image_ref() { # <service> <tag>
    printf '%s/kronn-%s:%s' "$IMAGE_PREFIX" "$1" "$2"
}

# The image is already on this machine (pulled earlier, or built locally).
image_is_local() { # <service> <tag>
    docker image inspect "$(image_ref "$1" "$2")" >/dev/null 2>&1
}

# Memory Docker can use, in MiB: `docker info` reports the VM that runs the
# containers (Docker Desktop, WSL), not the host the CLI runs on. Empty when it
# cannot be measured.
docker_mem_mib() {
    local bytes
    bytes="$(docker info --format '{{.MemTotal}}' 2>/dev/null | tr -d '[:space:]' || true)"
    case "$bytes" in
        ''|*[!0-9]*) return 0 ;;
    esac
    echo $((bytes / 1048576))
}

# Swap readable from here, in MiB. On a native Linux or WSL engine this is the
# swap the build would use; Docker Desktop's VM swap is not readable (0).
swap_mib() {
    local kb
    kb="$(sed -n 's/^SwapTotal:[[:space:]]*\([0-9][0-9]*\) kB.*/\1/p' "${KRONN_MEMINFO:-/proc/meminfo}" 2>/dev/null | head -n 1 || true)"
    echo $(( ${kb:-0} / 1024 ))
}

gib() { # MiB -> one-decimal GiB (C locale: a decimal point, never a comma)
    LC_ALL=C awk -v mib="$1" 'BEGIN { printf "%.1f", mib / 1024 }'
}

print_alternatives() {
    local tag="$1"
    info "  Instead of compiling here:"
    info "    1. Use the published image (nothing to compile, just a download):"
    info "         docker pull ghcr.io/docroms/kronn-backend:${tag}"
    info "       then run ${CYAN}make start${RESET} again (without KRONN_BUILD_FROM_SOURCE). If the pull is"
    info "       denied or the tag is unknown, this version has no published image yet: use option 2 or 3."
    info "    2. The desktop installer for your platform (Windows .exe, macOS .dmg, Linux .deb):"
    info "         ${RELEASES_URL}"
    info "    3. A bigger machine: ${CYAN}./kronn start-dev${RESET} compiles natively and peaks at ~10 GB too."
    info "       On Docker Desktop, raise Settings > Resources > Memory; on WSL, raise"
    info "       \`memory=\` in %UserProfile%\\.wslconfig."
}

# Refuse before compiling when the build cannot fit; warn when it is tight.
memory_guard() {
    local tag="$1" reason="$2"
    local mem swap total min
    min=$(( ${KRONN_BUILD_MIN_GB:-$BUILD_NEED_GB} * 1024 ))

    mem="$(docker_mem_mib)"
    if [ -z "$mem" ]; then
        info "${YELLOW}  Could not measure the memory Docker can use; building without the memory check.${RESET}"
        return 0
    fi
    swap="$(swap_mib)"
    total=$((mem + swap))

    if [ "$total" -lt "$min" ]; then
        info "${RED}✗ Not enough memory to build Kronn from source.${RESET}"
        info "  ${reason}"
        info "  Docker can use $(gib "$mem") GiB of RAM + $(gib "$swap") GiB of swap = $(gib "$total") GiB;"
        info "  compiling the backend peaks at ~${FAST_PEAK_GB} GiB and needs ~$((min / 1024)) GiB (${SINGLE_THREAD_GB} GiB, 12 min, with one codegen thread)."
        info "  The compiler would be killed (\"cannot allocate memory\") after minutes of work, so nothing was started."
        info ""
        print_alternatives "$tag"
        info ""
        if [ "${KRONN_FORCE_BUILD:-0}" = "1" ]; then
            info "${YELLOW}  KRONN_FORCE_BUILD=1: building anyway.${RESET}"
            return 0
        fi
        info "  To try anyway: ${CYAN}KRONN_FORCE_BUILD=1 make start-build${RESET}"
        return 1
    fi
}

TAG="$(image_tag)"
export KRONN_IMAGE_TAG="$TAG"
export CARGO_PROFILE="${CARGO_PROFILE:-fast}"

to_build=()
if [ "${KRONN_BUILD_FROM_SOURCE:-0}" = "1" ]; then
    reason="Source build requested (KRONN_BUILD_FROM_SOURCE=1 / make start-build)."
    to_build=(backend frontend)
else
    reason="The published image ${TAG} could not be pulled (VERSION not published yet, offline, or package not public)."
    info "${GREEN}▸ Pulling the published Kronn ${TAG} images (no compile)...${RESET}"
    for service in backend frontend; do
        if $COMPOSE pull "$service"; then
            continue
        fi
        if image_is_local "$service" "$TAG"; then
            # An offline restart, or a checkout built earlier: the image is
            # here, nothing needs to be fetched or compiled.
            info "${YELLOW}  Could not pull $(image_ref "$service" "$TAG"); starting the copy already on this machine.${RESET}"
        else
            info "${YELLOW}  Could not pull ${service}:${TAG} and no local copy exists; it will be built from source.${RESET}"
            to_build+=("$service")
        fi
    done
fi

if [ "${#to_build[@]}" -eq 0 ]; then
    $COMPOSE up -d --no-build
    exit 0
fi

case " ${to_build[*]} " in
    *" backend "*) memory_guard "$TAG" "$reason" ;;
esac

# One service built here, the other published: they may not be the same code.
if [ "${#to_build[@]}" -eq 1 ]; then
    info "${YELLOW}  Only ${to_build[0]} is built from this checkout; the other service runs the published ${TAG} image. They may differ.${RESET}"
fi

info "${GREEN}▸ Building ${to_build[*]} from source (profile ${CARGO_PROFILE})...${RESET}"
$COMPOSE build "${to_build[@]}"
$COMPOSE up -d --no-build

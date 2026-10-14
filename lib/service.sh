#!/usr/bin/env bash
# ─── Kronn background service (per-user launchd / systemd --user) ───────────
# Compatible Bash 3.2+ (macOS default): no associative arrays, no ${v,,},
# no `local -n`, empty arrays are always expanded as ${a[@]+"${a[@]}"}.
#
# Runs the native Kronn backend as a per-user service that serves the API and
# the web UI on :3140 with no terminal. The service NEVER runs from the
# developer checkout: it runs a staged, immutable release under
# $KRONN_SERVICE_HOME/releases/<version>, reached through the `current`
# symlink, so a `git pull`, a branch switch or a `cargo clean` in the checkout
# cannot affect it. Layout, identity and function names are a contract shared
# with lib/service-update.sh (see docs/operations/service.md).
#
#   $KRONN_SERVICE_HOME            (default ~/.kronn/service)
#     src/                         git clone used only by the service
#     releases/<version>/          bin/kronn  ui/  scripts/  cli/  VERSION  SOURCE_SHA
#     current -> releases/<version>
#     update.lock
#
# Every call to the service manager goes through KRONN_LAUNCHCTL /
# KRONN_SYSTEMCTL (and KRONN_JOURNALCTL for logs) so tests can inject fakes.
# Nothing here ever touches the Kronn data directory.

KRONN_SERVICE_LABEL="com.kronn.service"
KRONN_SERVICE_UNIT_NAME="kronn"
KRONN_SERVICE_DEFAULT_REPO="https://github.com/DocRoms/Kronn.git"

# ─── Identity & paths (pure) ────────────────────────────────────────────────

service_home()  { printf '%s\n' "${KRONN_SERVICE_HOME:-$HOME/.kronn/service}"; }
service_label() { printf '%s\n' "$KRONN_SERVICE_LABEL"; }

# launchd | systemd | unsupported. KRONN_SERVICE_KIND forces the answer (tests).
service_kind() {
    if [[ -n "${KRONN_SERVICE_KIND:-}" ]]; then
        printf '%s\n' "$KRONN_SERVICE_KIND"
        return 0
    fi
    case "$(uname -s 2>/dev/null)" in
        Darwin) echo launchd ;;
        Linux)
            if command -v "$(_service_systemctl_bin)" >/dev/null 2>&1; then
                echo systemd
            else
                echo unsupported
            fi
            ;;
        *) echo unsupported ;;
    esac
}

_service_launchctl_bin() { printf '%s\n' "${KRONN_LAUNCHCTL:-launchctl}"; }
_service_systemctl_bin() { printf '%s\n' "${KRONN_SYSTEMCTL:-systemctl}"; }
_service_journalctl_bin() { printf '%s\n' "${KRONN_JOURNALCTL:-journalctl}"; }

_service_launchctl() { "$(_service_launchctl_bin)" "$@"; }
_service_systemctl() { "$(_service_systemctl_bin)" --user "$@"; }
_service_domain()    { printf 'gui/%s\n' "$(id -u)"; }
_service_target()    { printf 'gui/%s/%s\n' "$(id -u)" "$KRONN_SERVICE_LABEL"; }

service_log_dir() { printf '%s\n' "$HOME/Library/Logs/Kronn"; }

service_unit_path() {
    case "$(service_kind)" in
        launchd) printf '%s\n' "$HOME/Library/LaunchAgents/${KRONN_SERVICE_LABEL}.plist" ;;
        systemd) printf '%s\n' "$HOME/.config/systemd/user/${KRONN_SERVICE_UNIT_NAME}.service" ;;
        *)       return 1 ;;
    esac
}

service_port() { printf '%s\n' "${KRONN_PORT:-3140}"; }
service_url()  { printf 'http://localhost:%s\n' "$(service_port)"; }

service_is_installed() {
    local unit
    unit="$(service_unit_path 2>/dev/null)" || return 1
    [[ -f "$unit" ]]
}

# launchd: the job is known to launchd (bootstrapped), running or not.
_service_launchd_loaded() {
    _service_launchctl print "$(_service_target)" >/dev/null 2>&1
}

service_is_running() {
    case "$(service_kind)" in
        launchd)
            local out
            out="$(_service_launchctl print "$(_service_target)" 2>/dev/null)" || return 1
            printf '%s\n' "$out" | grep -Eq 'state = running'
            ;;
        systemd)
            _service_systemctl is-active --quiet "$KRONN_SERVICE_UNIT_NAME" 2>/dev/null
            ;;
        *) return 1 ;;
    esac
}

# ─── Unit rendering (pure: stdout only) ─────────────────────────────────────

_service_xml_escape() {
    printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'
}

# systemd double-quoted value: escape \ and " ; % is a specifier prefix.
_service_systemd_quote() {
    local v="$1"
    v="${v//\\/\\\\}"
    v="${v//\"/\\\"}"
    v="${v//%/%%}"
    printf '"%s"' "$v"
}

# KEY=VALUE pairs of the service environment, one per line. Paths always go
# through <home>/current, never a versioned release dir, so a switch of the
# symlink is all an update needs. KRONN_DOCS_SIDECAR is a prebuilt bundle a
# source build does not produce: it is only set when the file exists.
_service_env_pairs() {
    local home="$1"
    printf 'KRONN_SERVICE=1\n'
    printf 'KRONN_UI_DIST=%s\n' "$home/current/ui"
    printf 'KRONN_DISC_INTROSPECTION_MCP=%s\n' "$home/current/scripts/disc-introspection-mcp.py"
    if [[ -f "$home/current/bin/kronn-docs-sidecar" ]]; then
        printf 'KRONN_DOCS_SIDECAR=%s\n' "$home/current/bin/kronn-docs-sidecar"
    fi
    printf 'HOME=%s\n' "$HOME"
}

service_render_plist() {
    local home="${1:-$(service_home)}" pair key val logs
    logs="$(service_log_dir)"
    cat <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$(_service_xml_escape "$KRONN_SERVICE_LABEL")</string>
    <key>ProgramArguments</key>
    <array>
        <string>$(_service_xml_escape "$home/current/bin/kronn")</string>
    </array>
    <key>EnvironmentVariables</key>
    <dict>
EOF
    while IFS= read -r pair; do
        [[ -n "$pair" ]] || continue
        key="${pair%%=*}"
        val="${pair#*=}"
        printf '        <key>%s</key>\n        <string>%s</string>\n' \
            "$(_service_xml_escape "$key")" "$(_service_xml_escape "$val")"
    done < <(_service_env_pairs "$home")
    cat <<EOF
    </dict>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>ProcessType</key>
    <string>Interactive</string>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>WorkingDirectory</key>
    <string>$(_service_xml_escape "$HOME")</string>
    <key>StandardOutPath</key>
    <string>$(_service_xml_escape "$logs/kronn.out.log")</string>
    <key>StandardErrorPath</key>
    <string>$(_service_xml_escape "$logs/kronn.err.log")</string>
</dict>
</plist>
EOF
}

service_render_systemd_unit() {
    local home="${1:-$(service_home)}" pair
    cat <<EOF
[Unit]
Description=Kronn background service
After=network.target

[Service]
Type=simple
ExecStart=$(_service_systemd_quote "$home/current/bin/kronn")
WorkingDirectory=~
Restart=on-failure
RestartSec=10
EOF
    while IFS= read -r pair; do
        [[ -n "$pair" ]] || continue
        printf 'Environment=%s\n' "$(_service_systemd_quote "$pair")"
    done < <(_service_env_pairs "$home")
    cat <<EOF

[Install]
WantedBy=default.target
EOF
}

# Write the unit for the current platform from the current service home.
service_write_unit() {
    local home unit
    home="$(service_home)"
    unit="$(service_unit_path)" || { fail "Unsupported platform for a Kronn service."; return 1; }
    mkdir -p "$(dirname "$unit")" || return 1
    case "$(service_kind)" in
        launchd)
            mkdir -p "$(service_log_dir)" || return 1
            service_render_plist "$home" > "$unit.tmp.$$" || return 1
            ;;
        systemd)
            service_render_systemd_unit "$home" > "$unit.tmp.$$" || return 1
            ;;
    esac
    mv -f "$unit.tmp.$$" "$unit"
}

# ─── Releases ───────────────────────────────────────────────────────────────

# A version names a directory: no separators, no leading dot.
_service_valid_version() {
    [[ "$1" =~ ^[A-Za-z0-9][A-Za-z0-9._+-]*$ ]]
}

# Build the release artifacts inside <src_dir>: the backend binary into
# <src_dir>/target/release/kronn and the frontend into <src_dir>/frontend/dist.
service_build_from_source() {
    local src="${1:-}"
    if [[ -z "$src" || ! -f "$src/backend/Cargo.toml" ]]; then
        fail "service_build_from_source: '$src' is not a Kronn source tree."
        return 1
    fi
    info "Building the backend (cargo build --release) — this takes a few minutes..."
    (cd "$src" && cargo build --release --manifest-path backend/Cargo.toml --target-dir "$src/target") || {
        fail "Backend build failed."
        return 1
    }
    info "Building the frontend (pnpm install --frozen-lockfile && pnpm build)..."
    (cd "$src/frontend" && pnpm install --frozen-lockfile && pnpm build) || {
        fail "Frontend build failed."
        return 1
    }
}

# Stage a built tree as releases/<version>. VERSION is written LAST: a release
# without it is incomplete and may be re-staged; a complete one is immutable.
service_stage_release() {
    local src="${1:-}" version="${2:-}" home dest sha
    if [[ -z "$src" || -z "$version" ]]; then
        fail "Usage: service_stage_release <src_dir> <version>"
        return 1
    fi
    if ! _service_valid_version "$version"; then
        fail "Invalid release version: '$version'"
        return 1
    fi
    home="$(service_home)"
    dest="$home/releases/$version"
    if [[ -f "$dest/VERSION" ]]; then
        fail "Release $version is already staged — releases are immutable."
        return 1
    fi
    local bin="$src/target/release/kronn" ui="$src/frontend/dist"
    if [[ ! -x "$bin" ]]; then
        fail "Backend binary not found: $bin (build first)."
        return 1
    fi
    if [[ ! -f "$ui/index.html" ]]; then
        fail "Frontend build not found: $ui/index.html (build first)."
        return 1
    fi
    if [[ ! -f "$src/kronn" || ! -d "$src/lib" ]]; then
        fail "CLI sources not found in $src (kronn, lib/)."
        return 1
    fi

    rm -rf "$dest"
    mkdir -p "$dest/bin" "$dest/ui" "$dest/scripts" "$dest/cli" || return 1
    {
        cp "$bin" "$dest/bin/kronn" &&
        cp -R "$ui/." "$dest/ui/" &&
        cp "$src/kronn" "$dest/cli/kronn" &&
        cp -R "$src/lib" "$dest/cli/lib" &&
        { [[ ! -d "$src/completions" ]] || cp -R "$src/completions" "$dest/cli/completions"; } &&
        { [[ ! -f "$src/VERSION" ]] || cp "$src/VERSION" "$dest/cli/VERSION"; }
    } || { rm -rf "$dest"; fail "Copying the build into $dest failed."; return 1; }
    chmod +x "$dest/bin/kronn" "$dest/cli/kronn"

    # Helper scripts the backend launches (MCP bridge, ...). Tests and CI
    # fixtures stay out of the release.
    local f
    if [[ -d "$src/backend/scripts" ]]; then
        for f in "$src/backend/scripts"/*; do
            [[ -f "$f" ]] || continue
            case "$(basename "$f")" in test_*) continue ;; esac
            cp "$f" "$dest/scripts/"
        done
    fi

    sha="$(git -C "$src" rev-parse HEAD 2>/dev/null || true)"
    printf '%s\n' "${sha:-unknown}" > "$dest/SOURCE_SHA"
    printf '%s\n' "$version" > "$dest/VERSION"
}

# Atomically repoint `current` at releases/<version>: build a temp symlink next
# to it, then rename(2) it over `current`. A plain `mv -f tmp current` would
# move the link INTO the directory `current` points to, hence -h (BSD) / -T
# (GNU); python3's os.replace is the fallback.
service_switch_current() {
    local version="${1:-}" home tmp
    if [[ -z "$version" ]] || ! _service_valid_version "$version"; then
        fail "Usage: service_switch_current <version>"
        return 1
    fi
    home="$(service_home)"
    if [[ ! -f "$home/releases/$version/VERSION" ]]; then
        fail "Release $version is not staged."
        return 1
    fi
    if [[ -e "$home/current" && ! -L "$home/current" ]]; then
        fail "$home/current exists and is not a symlink — refusing to replace it."
        return 1
    fi
    tmp="$home/.current.tmp.$$"
    rm -f "$tmp"
    ln -s "releases/$version" "$tmp" || return 1
    if ! _service_rename_over "$tmp" "$home/current"; then
        rm -f "$tmp"
        fail "Could not switch $home/current to $version."
        return 1
    fi
}

_service_rename_over() {
    local from="$1" to="$2"
    case "$(uname -s 2>/dev/null)" in
        Darwin|*BSD) mv -fh "$from" "$to" 2>/dev/null && return 0 ;;
        *)           mv -fT "$from" "$to" 2>/dev/null && return 0 ;;
    esac
    if command -v python3 >/dev/null 2>&1; then
        python3 -c 'import os, sys; os.replace(sys.argv[1], sys.argv[2])' "$from" "$to"
        return $?
    fi
    return 1
}

service_current_version() {
    local f
    f="$(service_home)/current/VERSION"
    [[ -f "$f" ]] || return 0
    tr -d '[:space:]' < "$f"
    echo
}

# Highest X.Y.Z tag of <src_dir> (non-semver tags are ignored); empty if none.
service_latest_release_tag() {
    local src="${1:-$(service_home)/src}"
    git -C "$src" tag --list 2>/dev/null \
        | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' \
        | sort -t. -k1,1n -k2,2n -k3,3n \
        | tail -n 1 || true
}

# ─── Service manager (launchctl / systemctl) ────────────────────────────────

service_load() {
    case "$(service_kind)" in
        launchd)
            local unit
            unit="$(service_unit_path)"
            # Re-bootstrap so an edited plist is picked up; bootout is async,
            # so a fresh bootstrap right after it is retried briefly.
            if _service_launchd_loaded; then
                _service_launchctl bootout "$(_service_target)" >/dev/null 2>&1 || true
            fi
            for _ in 1 2 3 4 5; do
                _service_launchctl bootstrap "$(_service_domain)" "$unit" && return 0
                sleep "${KRONN_SERVICE_RETRY_DELAY:-1}"
            done
            return 1
            ;;
        systemd)
            _service_systemctl daemon-reload || return 1
            _service_systemctl enable --now "$KRONN_SERVICE_UNIT_NAME"
            ;;
        *) return 1 ;;
    esac
}

service_unload() {
    case "$(service_kind)" in
        launchd)
            _service_launchd_loaded || return 0
            _service_launchctl bootout "$(_service_target)"
            ;;
        systemd)
            _service_systemctl stop "$KRONN_SERVICE_UNIT_NAME"
            ;;
        *) return 1 ;;
    esac
}

service_restart() {
    case "$(service_kind)" in
        launchd)
            if _service_launchd_loaded; then
                _service_launchctl kickstart -k "$(_service_target)"
            else
                service_load
            fi
            ;;
        systemd)
            _service_systemctl restart "$KRONN_SERVICE_UNIT_NAME"
            ;;
        *) return 1 ;;
    esac
}

# Poll /api/health until it answers ok with the expected version.
service_wait_healthy() {
    local expected="${1:-}" timeout="${2:-60}" url body got deadline
    url="http://127.0.0.1:$(service_port)/api/health"
    deadline=$((SECONDS + timeout))
    while :; do
        body="$(curl -fsS --max-time 2 "$url" 2>/dev/null || true)"
        if printf '%s' "$body" | grep -Eq '"ok" *: *true'; then
            got="$(printf '%s' "$body" | sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p')"
            if [[ -z "$expected" || "$got" == "$expected" ]]; then
                return 0
            fi
        fi
        (( SECONDS >= deadline )) && return 1
        sleep "${KRONN_SERVICE_HEALTH_INTERVAL:-1}"
    done
}

# ─── Coexistence with start-dev / start / web ───────────────────────────────

# Set by service_offer_stop when it stopped the service on behalf of a dev run.
KRONN_SERVICE_STOPPED_BY_DEV=0

# A running service owns :3140 and the data-dir lock. Offer to stop it (unload:
# KeepAlive would respawn a merely killed process). Returns 0 when the caller
# may proceed (nothing running, or stopped), 1 when the user declined.
# $1 = "reload" asks the caller's EXIT trap to bring the service back
# (service_resume_after_dev).
service_offer_stop() {
    local mode="${1:-}"
    service_is_installed || return 0
    service_is_running || return 0
    warn "Kronn is already running as a background service ($(service_url))."
    if ! ask_yn "Stop the service while this command runs?"; then
        return 1
    fi
    if ! service_unload; then
        fail "Could not stop the service."
        return 1
    fi
    success "Service stopped."
    if [[ "$mode" == "reload" ]]; then
        KRONN_SERVICE_STOPPED_BY_DEV=1
        printf "  ${DIM}It is started again when you exit (or run ${BOLD}kronn service start${RESET}${DIM}).${RESET}\n"
    else
        printf "  ${DIM}Start it again with ${BOLD}kronn service start${RESET}${DIM}.${RESET}\n"
    fi
    return 0
}

# EXIT-trap helper: reload the service if service_offer_stop stopped it.
service_resume_after_dev() {
    [[ "${KRONN_SERVICE_STOPPED_BY_DEV:-0}" == "1" ]] || return 0
    KRONN_SERVICE_STOPPED_BY_DEV=0
    service_load >/dev/null 2>&1 || true
}

# ─── Commands ───────────────────────────────────────────────────────────────

_service_confirm() {
    [[ "${_SERVICE_ASSUME_YES:-0}" == "1" ]] && return 0
    ask_yn "$1"
}

_service_require_supported() {
    if [[ "$(service_kind)" == "unsupported" ]]; then
        fail "Background service is supported on macOS (launchd) and Linux (systemd --user) only."
        return 1
    fi
}

_service_require_installed() {
    _service_require_supported || return 1
    if ! service_is_installed; then
        fail "The Kronn service is not installed. Run: kronn service install"
        return 1
    fi
}

# Space-separated list of missing build prerequisites (empty = all present).
service_missing_tools() {
    local t missing=""
    for t in git cargo node pnpm; do
        command -v "$t" >/dev/null 2>&1 || missing="$missing $t"
    done
    printf '%s\n' "${missing# }"
}

_service_clone_url() {
    local url=""
    if [[ -n "${KRONN_DIR:-}" ]]; then
        url="$(git -C "$KRONN_DIR" remote get-url origin 2>/dev/null || true)"
    fi
    printf '%s\n' "${url:-$KRONN_SERVICE_DEFAULT_REPO}"
}

cmd_service_install() {
    local from_checkout=0 arg
    _SERVICE_ASSUME_YES=0
    for arg in "$@"; do
        case "$arg" in
            --from-checkout) from_checkout=1 ;;
            -y|--yes) _SERVICE_ASSUME_YES=1 ;;
            *) fail "Unknown option for 'service install': $arg"; return 1 ;;
        esac
    done

    step "Installing the Kronn background service"
    _service_require_supported || return 1

    # Toolchains often live outside a non-interactive PATH (see cmd_start_dev).
    [[ -d "$HOME/.cargo/bin" ]] && PATH="$(path_append_missing "$PATH" "$HOME/.cargo/bin")"
    [[ -d "/opt/homebrew/bin" ]] && PATH="$(path_append_missing "$PATH" "/opt/homebrew/bin")"
    export PATH

    local missing
    missing="$(service_missing_tools)"
    if [[ -n "$missing" ]]; then
        fail "Missing prerequisite(s): $missing"
        printf "  ${DIM}The service is built from source: git, cargo (rustup), node >= 24 and pnpm are required.${RESET}\n"
        return 1
    fi

    local home src version base_version
    home="$(service_home)"
    mkdir -p "$home/releases" || return 1

    if service_is_installed; then
        warn "The service is already installed (current: $(service_current_version))."
        _service_confirm "Reinstall it?" || { info "Aborted."; return 0; }
    fi

    if [[ "$from_checkout" == "1" ]]; then
        src="$KRONN_DIR"
        if [[ ! -d "$src/.git" && ! -f "$src/.git" ]] || [[ ! -f "$src/backend/Cargo.toml" ]]; then
            fail "--from-checkout needs a Kronn git checkout; $src is not one."
            return 1
        fi
        local sha
        sha="$(git -C "$src" rev-parse --short HEAD 2>/dev/null || echo unknown)"
        base_version="$(tr -d '[:space:]' < "$src/VERSION" 2>/dev/null || true)"
        base_version="${base_version:-0.0.0}"
        version="${base_version}-dev-${sha}"
        # Uncommitted changes are built too: give them their own release name.
        if [[ -n "$(git -C "$src" status --porcelain 2>/dev/null)" ]]; then
            version="${version}-dirty$(date +%Y%m%d%H%M%S)"
        fi
        warn "Dev install from the checkout HEAD ($sha) — for testing, not a released version."
    else
        src="$home/src"
        if [[ -d "$src/.git" ]]; then
            info "Updating the service clone..."
            git -C "$src" fetch --tags --quiet || { fail "git fetch failed in $src"; return 1; }
        else
            local url
            url="$(_service_clone_url)"
            info "Cloning $url ..."
            git clone --quiet "$url" "$src" || { fail "git clone failed."; return 1; }
        fi
        version="$(service_latest_release_tag "$src")"
        if [[ -z "$version" ]]; then
            fail "No release tag (X.Y.Z) found in the clone. Use 'kronn service install --from-checkout'."
            return 1
        fi
        base_version="$version"
        info "Latest release: $version"
        git -C "$src" checkout --quiet --force "$version" || { fail "git checkout $version failed."; return 1; }
    fi

    if [[ -f "$home/releases/$version/VERSION" ]]; then
        info "Release $version is already staged — reusing it."
    else
        service_build_from_source "$src" || return 1
        service_stage_release "$src" "$version" || return 1
        success "Staged release $version"
    fi
    service_switch_current "$version" || return 1
    service_write_unit || return 1
    success "Service definition written: $(service_unit_path)"

    service_load || { fail "Could not load the service ($(service_label))."; return 1; }
    info "Waiting for the backend to answer on $(service_url) ..."
    if service_wait_healthy "$base_version" "${KRONN_SERVICE_HEALTH_TIMEOUT:-90}"; then
        success "Kronn $base_version is running."
    else
        fail "The service did not become healthy in time."
        printf "  ${DIM}Check: ${BOLD}kronn service logs${RESET}\n"
        return 1
    fi

    _service_offer_cli_link "$home"

    echo
    success "Kronn runs as a background service."
    printf "  ${BOLD}UI → ${GREEN}%s${RESET}\n" "$(service_url)"
    printf "  ${DIM}Manage it with: kronn service status | logs | restart | stop | uninstall${RESET}\n"
}

# Offer to point ~/.local/bin/kronn at the staged CLI (so `kronn` keeps working
# from anywhere and follows `current` across updates).
_service_offer_cli_link() {
    local home="$1" link_dir="$HOME/.local/bin" target
    local link="$link_dir/kronn"
    target="$home/current/cli/kronn"
    [[ -f "$target" ]] || return 0
    if [[ -L "$link" && "$(readlink "$link")" == "$target" ]]; then
        return 0
    fi
    if _service_confirm "Point $link at the service CLI ($target)?"; then
        mkdir -p "$link_dir" && ln -sfn "$target" "$link"
        success "Symlink: $link -> $target"
        case ":$PATH:" in
            *":$link_dir:"*) : ;;
            *) warn "$link_dir is not in your PATH." ;;
        esac
    fi
}

cmd_service_start() {
    _service_require_installed || return 1
    if service_is_running; then
        info "The service is already running."
        return 0
    fi
    service_load || { fail "Could not start the service."; return 1; }
    success "Service started — $(service_url)"
}

cmd_service_stop() {
    _service_require_installed || return 1
    if ! service_is_running && [[ "$(service_kind)" == "systemd" ]]; then
        info "The service is not running."
        return 0
    fi
    service_unload || { fail "Could not stop the service."; return 1; }
    success "Service stopped (it starts again at your next login)."
}

cmd_service_restart() {
    _service_require_installed || return 1
    service_restart || { fail "Could not restart the service."; return 1; }
    success "Service restarted."
}

cmd_service_status() {
    _service_require_supported || return 1
    local installed="no" running="no" ver health="unreachable" body got
    service_is_installed && installed="yes"
    service_is_running && running="yes"
    ver="$(service_current_version)"
    printf "  ${BOLD}Service${RESET}    %s (%s)\n" "$(service_label)" "$(service_kind)"
    printf "  ${BOLD}Installed${RESET}  %s\n" "$installed"
    printf "  ${BOLD}Running${RESET}    %s\n" "$running"
    printf "  ${BOLD}Version${RESET}    %s\n" "${ver:-n/a}"
    printf "  ${BOLD}URL${RESET}        %s\n" "$(service_url)"
    body="$(curl -fsS --max-time 2 "http://127.0.0.1:$(service_port)/api/health" 2>/dev/null || true)"
    if printf '%s' "$body" | grep -Eq '"ok" *: *true'; then
        got="$(printf '%s' "$body" | sed -n 's/.*"version" *: *"\([^"]*\)".*/\1/p')"
        health="ok (backend ${got:-unknown})"
    fi
    printf "  ${BOLD}Health${RESET}     %s\n" "$health"
}

cmd_service_logs() {
    _service_require_installed || return 1
    local follow=1 arg
    for arg in "$@"; do
        case "$arg" in
            --no-follow) follow=0 ;;
            *) fail "Unknown option for 'service logs': $arg"; return 1 ;;
        esac
    done
    case "$(service_kind)" in
        launchd)
            local d out err
            d="$(service_log_dir)"
            out="$d/kronn.out.log"
            err="$d/kronn.err.log"
            mkdir -p "$d"
            touch "$out" "$err"
            if [[ "$follow" == "1" ]]; then
                printf "  ${DIM}Tailing %s (Ctrl+C detaches — the service keeps running).${RESET}\n" "$d"
                tail -n 100 -F "$out" "$err"
            else
                tail -n 100 "$out" "$err"
            fi
            ;;
        systemd)
            if [[ "$follow" == "1" ]]; then
                "$(_service_journalctl_bin)" --user -u "$KRONN_SERVICE_UNIT_NAME" -f
            else
                "$(_service_journalctl_bin)" --user -u "$KRONN_SERVICE_UNIT_NAME" -n 100 --no-pager
            fi
            ;;
    esac
}

cmd_service_uninstall() {
    local purge=0 arg
    _SERVICE_ASSUME_YES=0
    for arg in "$@"; do
        case "$arg" in
            -y|--yes) _SERVICE_ASSUME_YES=1 ;;
            --purge) purge=1 ;;
            *) fail "Unknown option for 'service uninstall': $arg"; return 1 ;;
        esac
    done
    _service_require_supported || return 1
    local home unit
    home="$(service_home)"
    unit="$(service_unit_path)"
    if ! service_is_installed && [[ ! -d "$home" ]]; then
        info "Nothing to uninstall."
        return 0
    fi
    _service_confirm "Uninstall the Kronn background service?" || { info "Aborted."; return 0; }

    service_unload >/dev/null 2>&1 || true
    if [[ "$(service_kind)" == "systemd" ]]; then
        _service_systemctl disable "$KRONN_SERVICE_UNIT_NAME" >/dev/null 2>&1 || true
    fi
    rm -f "$unit"
    if [[ "$(service_kind)" == "systemd" ]]; then
        _service_systemctl daemon-reload >/dev/null 2>&1 || true
    fi
    success "Service unloaded and definition removed."

    # Drop the PATH symlink only if it points into the service home.
    local link="$HOME/.local/bin/kronn"
    if [[ -L "$link" && "$(readlink "$link")" == "$home/"* ]]; then
        rm -f "$link"
        info "Removed $link (it pointed to the service CLI)."
    fi

    if [[ -d "$home" ]]; then
        local delete=0
        if [[ "$purge" == "1" ]]; then
            delete=1
        elif [[ "$_SERVICE_ASSUME_YES" != "1" ]] && ask_yn "Also delete $home (releases and source clone)?"; then
            delete=1
        fi
        if [[ "$delete" == "1" ]]; then
            case "$home" in
                ""|"/"|"$HOME"|"$HOME/") fail "Refusing to delete '$home'."; return 1 ;;
            esac
            rm -rf "$home"
            success "Deleted $home"
        else
            info "Kept $home"
        fi
    fi
    printf "  ${DIM}Your Kronn data (database, config) was not touched.${RESET}\n"
}

cmd_service_usage() {
    printf "${BOLD}Usage:${RESET} kronn service <command>\n\n"
    printf "  ${CYAN}install${RESET} [--from-checkout] [--yes]   Build the latest release and register the service\n"
    printf "  ${CYAN}start${RESET} | ${CYAN}stop${RESET} | ${CYAN}restart${RESET}              Control the service\n"
    printf "  ${CYAN}status${RESET}                              Installed, running, version, URL, health\n"
    printf "  ${CYAN}logs${RESET} [--no-follow]                  Tail the service logs\n"
    printf "  ${CYAN}uninstall${RESET} [--yes] [--purge]         Remove the service (data dir is never touched)\n"
}

cmd_service() {
    local sub="${1:-}"
    shift 2>/dev/null || true
    case "$sub" in
        install)   cmd_service_install "$@" ;;
        start)     cmd_service_start ;;
        stop)      cmd_service_stop ;;
        restart)   cmd_service_restart ;;
        status)    cmd_service_status ;;
        logs)      cmd_service_logs "$@" ;;
        uninstall) cmd_service_uninstall "$@" ;;
        update|rollback)
            if declare -F "cmd_service_$sub" >/dev/null 2>&1; then
                "cmd_service_$sub" "$@"
            else
                fail "'kronn service $sub' is not available in this version."
                return 1
            fi
            ;;
        ""|help|--help|-h) cmd_service_usage ;;
        *)
            fail "Unknown service command: $sub"
            cmd_service_usage
            return 1
            ;;
    esac
}

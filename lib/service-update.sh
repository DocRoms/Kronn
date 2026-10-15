#!/usr/bin/env bash
# ─── Kronn service: update and rollback ─────────────────────────────────────
# Compatible Bash 3.2+ (macOS default): no associative arrays, no ${v,,},
# no `local -n`, empty arrays are always expanded as ${a[@]+"${a[@]}"}.
#
# `kronn service update` fetches the service clone, builds the target release
# next to the running one, waits for in-flight work to finish, switches the
# `current` symlink, restarts and health-checks. A release that does not come
# up healthy is rolled back automatically (symlink, and the database when the
# new version migrated it). Everything is built on lib/service.sh; the running
# CLI lives in the OLD release directory, which is never modified, so the
# update can safely replace the script that is executing it.
#
# Exit codes: 0 ok / already up to date, 1 failure (including a failed update
# that was rolled back), 10 `--check` found an update (see docs/operations/service.md).
#
# State kept under $KRONN_SERVICE_HOME:
#   update.lock/        mkdir lock holding a `pid` file
#   previous            version `kronn service rollback` returns to
#   update.state        from/to versions and the backup mtime before the update,
#                       to tell afterwards whether the new version migrated

# ─── Helpers ────────────────────────────────────────────────────────────────

_su_valid_semver() { [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; }

# The backend reports its own version: a `-dev-<sha>` release is that of its
# VERSION file.
_su_backend_version() { printf '%s\n' "${1%%-dev-*}"; }

# Seconds since the epoch of a file's last modification; 0 when absent.
# GNU first: on coreutils `-f` means "file system status", so `stat -f %m FILE`
# prints the fs block and exits 1 instead of failing cleanly. BSD/macOS rejects
# `-c` without printing anything, so this order is safe on both.
_su_mtime() {
    [[ -e "$1" ]] || { echo 0; return 0; }
    stat -c %Y "$1" 2>/dev/null || stat -f %m "$1" 2>/dev/null || echo 0
}

# Directory holding the database. Mirrors backend/src/core/config.rs
# (config_dir: KRONN_DATA_DIR, else ProjectDirs com.kronn.kronn).
service_data_dir() {
    if [[ -n "${KRONN_DATA_DIR:-}" ]]; then
        printf '%s\n' "$KRONN_DATA_DIR"
    elif [[ "$(uname -s 2>/dev/null)" == "Darwin" ]]; then
        printf '%s\n' "$HOME/Library/Application Support/com.kronn.kronn"
    else
        printf '%s\n' "${XDG_CONFIG_HOME:-$HOME/.config}/kronn"
    fi
}

# backend/src/db/mod.rs opens <data dir>/kronn.db; the pre-migration copy is
# <db>.backup (db/migrations.rs backup_before_migration).
service_db_path()     { printf '%s/kronn.db\n' "$(service_data_dir)"; }
service_db_backup()   { printf '%s.backup\n' "$(service_db_path)"; }

_su_previous_file() { printf '%s/previous\n' "$(service_home)"; }
_su_state_file()    { printf '%s/update.state\n' "$(service_home)"; }

_su_read_previous() {
    local f
    f="$(_su_previous_file)"
    [[ -f "$f" ]] || return 0
    tr -d '[:space:]' < "$f"
    echo
}

# ─── Lock ───────────────────────────────────────────────────────────────────

_su_lock_dir() { printf '%s/update.lock\n' "$(service_home)"; }

_su_lock_acquire() {
    local dir pid
    dir="$(_su_lock_dir)"
    mkdir -p "$(service_home)" || return 1
    for _ in 1 2; do
        if mkdir "$dir" 2>/dev/null; then
            printf '%s\n' "$$" > "$dir/pid"
            return 0
        fi
        pid="$(cat "$dir/pid" 2>/dev/null || true)"
        if [[ -z "$pid" ]]; then
            # The owner may sit between mkdir and writing its pid.
            sleep 1
            pid="$(cat "$dir/pid" 2>/dev/null || true)"
        fi
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            fail "Another 'kronn service update/rollback' is running (pid $pid)."
            return 1
        fi
        warn "Clearing a stale update lock (pid ${pid:-unknown} is gone)."
        rm -rf "$dir"
    done
    fail "Could not take the update lock $dir"
    return 1
}

_su_lock_release() {
    local dir pid
    dir="$(_su_lock_dir)"
    pid="$(cat "$dir/pid" 2>/dev/null || true)"
    [[ "$pid" == "$$" ]] && rm -rf "$dir"
    return 0
}

# Run "$@" under the lock, releasing it on any exit path.
_su_with_lock() {
    local rc
    _su_lock_acquire || return 1
    trap '_su_lock_release; exit 130' INT TERM
    "$@"
    rc=$?
    trap - INT TERM
    _su_lock_release
    return $rc
}

# ─── Drain ──────────────────────────────────────────────────────────────────

# Count the JSON array length / matching entries on stdin: $1 = workflows|plain.
_su_json_count() {
    local mode="$1"
    if command -v jq >/dev/null 2>&1; then
        if [[ "$mode" == "workflows" ]]; then
            jq -r '[.data[]? | select(.last_run.status == "Running" or .last_run.status == "Pending")] | length' 2>/dev/null
        else
            jq -r '[.data[]?] | length' 2>/dev/null
        fi
    else
        python3 -I -c '
import sys, json
mode = sys.argv[1]
try:
    data = json.load(sys.stdin).get("data") or []
except Exception:
    print("error"); sys.exit(0)
if mode == "workflows":
    data = [w for w in data if ((w.get("last_run") or {}).get("status") in ("Running", "Pending"))]
print(len(data))
' "$mode" 2>/dev/null
    fi
}

# Is anything in flight on the running backend? Prints a summary line.
# Returns 0 = busy, 1 = idle, 2 = API unreachable (treated as idle, with a
# warning). Tests override this function.
#
# Two read-only routes of the backend, both authenticated like the rest of the
# CLI (lib/api-client.sh: Bearer token from ~/.config/kronn/.auth_token):
#   GET /api/workflows           each summary carries its latest run; a run in
#                                Running or Pending is in flight (the same
#                                reading as the `workflow_active_runs` tool of
#                                backend/scripts/disc-introspection-mcp.py).
#                                WaitingApproval is NOT counted: it waits for
#                                a human, would stall every update for the full
#                                drain timeout, and its state lives in the DB.
#   GET /api/discussions/running ids of discussions with a live agent stream.
service_update_active_work() {
    local base wf disc nwf nd
    base="http://127.0.0.1:$(service_port)"
    wf="$(KRONN_API_BASE="$base" kronn_api GET /workflows)" || return 2
    disc="$(KRONN_API_BASE="$base" kronn_api GET /discussions/running)" || return 2
    nwf="$(printf '%s' "$wf" | _su_json_count workflows)"
    nd="$(printf '%s' "$disc" | _su_json_count plain)"
    [[ "$nwf" =~ ^[0-9]+$ && "$nd" =~ ^[0-9]+$ ]] || return 2
    if (( nwf + nd > 0 )); then
        printf '%s workflow run(s), %s running agent discussion(s)\n' "$nwf" "$nd"
        return 0
    fi
    return 1
}

# Wait for in-flight work. Returns 0 to proceed, 1 when the user aborts.
_su_drain() {
    local timeout="${KRONN_SERVICE_DRAIN_TIMEOUT:-1800}" interval="${KRONN_SERVICE_DRAIN_INTERVAL:-10}"
    local deadline summary rc announced=0
    deadline=$((SECONDS + timeout))
    while :; do
        summary="$(service_update_active_work)"
        rc=$?
        case "$rc" in
            0) ;;
            2)
                warn "Could not ask the running service what is active (API unreachable) — assuming nothing is."
                return 0
                ;;
            *)
                [[ "$announced" == "1" ]] && success "Nothing is running anymore."
                return 0
                ;;
        esac
        if [[ "$announced" == "0" ]]; then
            info "Waiting for in-flight work to finish before restarting: $summary"
            printf "  ${DIM}Timeout %ss (KRONN_SERVICE_DRAIN_TIMEOUT); --force skips the wait.${RESET}\n" "$timeout"
            announced=1
        fi
        if (( SECONDS >= deadline )); then
            warn "Still active after ${timeout}s: $summary"
            if [[ "${_SERVICE_ASSUME_YES:-0}" == "1" ]]; then
                warn "--yes: restarting anyway."
                return 0
            fi
            if ask_yn "Restart anyway? Running work will be interrupted."; then
                return 0
            fi
            return 1
        fi
        sleep "$interval"
    done
}

# ─── State, prune, changelog ────────────────────────────────────────────────

_su_write_state() { # <from> <to> <backup_mtime>
    printf 'from=%s\nto=%s\nbackup_mtime=%s\n' "$1" "$2" "$3" > "$(_su_state_file).tmp.$$" &&
        mv -f "$(_su_state_file).tmp.$$" "$(_su_state_file)"
}

_su_state_get() { # <key>
    local f
    f="$(_su_state_file)"
    [[ -f "$f" ]] || return 0
    sed -n "s/^$1=//p" "$f" | head -n 1
}

# Keep the 3 most recent releases by semver plus current and previous.
_su_prune_releases() {
    local home dir name keep_current keep_previous count=0 removed=0
    home="$(service_home)"
    keep_current="$(service_current_version)"
    keep_previous="$(_su_read_previous)"
    # Newest first. Names are X.Y.Z or X.Y.Z-dev-...: sort on the numeric parts.
    while IFS= read -r name; do
        [[ -n "$name" ]] || continue
        count=$((count + 1))
        (( count <= 3 )) && continue
        [[ "$name" == "$keep_current" || "$name" == "$keep_previous" ]] && continue
        _service_valid_version "$name" || continue
        rm -rf "${home:?}/releases/$name"
        removed=$((removed + 1))
        info "Pruned release $name"
    done < <(
        for dir in "$home"/releases/*/; do
            [[ -f "${dir}VERSION" ]] || continue
            basename "$dir"
        done | sort -t. -k1,1nr -k2,2nr -k3,3nr
    )
    return 0
}

# The `## [<version>]` block of a changelog file.
_su_changelog_section() { # <version> <file>
    [[ -f "$2" ]] || return 1
    awk -v h="## [$1]" '
        index($0, h) == 1 { p = 1; print; next }
        /^## \[/ { p = 0 }
        p
    ' "$2"
}

_su_print_changelog() { # <version> <src>
    local v="$1" src="$2" f out=""
    for f in "$(service_home)/releases/$v/CHANGELOG.md" "$src/CHANGELOG.md"; do
        out="$(_su_changelog_section "$v" "$f" 2>/dev/null || true)"
        [[ -n "$out" ]] && break
    done
    [[ -n "$out" ]] || return 0
    echo
    printf "${BOLD}What's new in %s${RESET}\n" "$v"
    printf '%s\n' "$out"
}

# ─── Rollback ───────────────────────────────────────────────────────────────

_su_undo_db_move() { # <db> <suffix>
    local side
    for side in "" -wal -shm; do
        [[ -e "$1.$2$side" ]] && mv "$1.$2$side" "$1$side"
    done
    return 0
}

# Move the live database (and its WAL/SHM side files) aside under a name that
# keeps the SQLite pairing, then put the pre-migration backup in its place.
_su_restore_db() { # <failed_version>
    local db backup suffix side
    db="$(service_db_path)"
    backup="$(service_db_backup)"
    suffix="failed-$1"
    if [[ -e "$db.$suffix" ]]; then
        suffix="$suffix-$(date +%Y%m%d%H%M%S)"
    fi
    for side in "" -wal -shm; do
        if [[ -e "$db$side" ]] && ! mv "$db$side" "$db.$suffix$side"; then
            _su_undo_db_move "$db" "$suffix"
            fail "Could not move the current database aside."
            return 1
        fi
    done
    if ! cp -p "$backup" "$db"; then
        _su_undo_db_move "$db" "$suffix"
        fail "Could not restore $backup"
        return 1
    fi
    success "Database restored from the pre-migration backup."
    printf "  ${DIM}The database written by %s was kept as %s${RESET}\n" "$1" "$db.$suffix"
}

# Back to <prev>. $3 = auto | manual. $4 = "migrated" when the DB must be
# restored. Returns 0 when the previous release is healthy again.
_su_rollback() { # <prev> <failed_version> <mode> <migrated:0|1>
    local prev="$1" failed="$2" mode="$3" migrated="$4"
    service_unload >/dev/null 2>&1 || true
    if [[ "$migrated" == "1" ]]; then
        if [[ ! -f "$(service_db_backup)" ]]; then
            warn "The new version migrated the database but no backup exists at $(service_db_backup)."
        else
            if [[ "$mode" == "manual" ]]; then
                warn "The database was migrated by $failed. Restoring the backup drops everything written since the update."
                if ! _service_confirm "Restore the pre-migration database backup?"; then
                    info "Aborted — the service is stopped; start it with: kronn service start"
                    return 1
                fi
            fi
            _su_restore_db "$failed" || return 1
        fi
    fi
    service_switch_current "$prev" || return 1
    service_load || { fail "Could not load the service after the rollback."; return 1; }
    info "Waiting for $prev to answer on $(service_url) ..."
    if service_wait_healthy "$(_su_backend_version "$prev")" "${KRONN_SERVICE_HEALTH_TIMEOUT:-90}"; then
        success "Rolled back to $prev."
        return 0
    fi
    fail "Release $prev did not become healthy after the rollback. Check: kronn service logs"
    return 1
}

cmd_service_rollback() {
    _SERVICE_ASSUME_YES=0
    local arg
    for arg in "$@"; do
        case "$arg" in
            -y|--yes) _SERVICE_ASSUME_YES=1 ;;
            *) fail "Unknown option for 'service rollback': $arg"; return 1 ;;
        esac
    done
    _service_require_installed || return 1
    _su_with_lock _su_rollback_manual
}

_su_rollback_manual() {
    local cur prev migrated=0 before now
    cur="$(service_current_version)"
    prev="$(_su_read_previous)"
    if [[ -z "$prev" || "$prev" == "$cur" ]]; then
        fail "There is no previous release to go back to."
        return 1
    fi
    if [[ ! -f "$(service_home)/releases/$prev/VERSION" ]]; then
        fail "Previous release $prev is no longer staged (pruned or removed)."
        return 1
    fi
    step "Rolling back ${cur:-?} -> $prev"
    if [[ "$(_su_state_get to)" == "$cur" ]]; then
        before="$(_su_state_get backup_mtime)"
        now="$(_su_mtime "$(service_db_backup)")"
        if [[ "$before" =~ ^[0-9]+$ ]] && (( now > before )); then
            migrated=1
        fi
    fi
    _su_rollback "$prev" "$cur" manual "$migrated" || return 1
    # A second `rollback` goes forward again; the update state is spent.
    printf '%s\n' "$cur" > "$(_su_previous_file)"
    rm -f "$(_su_state_file)"
}

# ─── Update ─────────────────────────────────────────────────────────────────

cmd_service_update() {
    local to="" check=0 force=0 arg
    _SERVICE_ASSUME_YES=0
    while [[ $# -gt 0 ]]; do
        arg="$1"; shift
        case "$arg" in
            --to)
                [[ $# -gt 0 ]] || { fail "--to needs a version (X.Y.Z)."; return 1; }
                to="$1"; shift ;;
            --to=*) to="${arg#--to=}" ;;
            --check) check=1 ;;
            --force) force=1 ;;
            -y|--yes) _SERVICE_ASSUME_YES=1 ;;
            *) fail "Unknown option for 'service update': $arg"; return 1 ;;
        esac
    done
    if [[ -n "$to" ]] && ! _su_valid_semver "$to"; then
        fail "--to expects a release version like 0.15.1 (got '$to')."
        return 1
    fi
    _service_require_installed || return 1
    _su_with_lock _su_update_locked "$to" "$check" "$force"
}

_su_update_locked() { # <to> <check> <force>
    local to="$1" check="$2" force="$3"
    local home src cur target prev old_previous backup_before
    home="$(service_home)"
    src="$home/src"
    cur="$(service_current_version)"
    if [[ -z "$cur" ]]; then
        fail "No current release ($home/current is missing). Run: kronn service install"
        return 1
    fi
    if [[ ! -d "$src/.git" ]]; then
        fail "The service clone $src is missing. Run: kronn service install"
        return 1
    fi

    step "Updating the Kronn service (current: $cur)"
    info "Fetching tags..."
    git -C "$src" fetch --tags --force --quiet origin || { fail "git fetch failed in $src"; return 1; }

    if [[ -n "$to" ]]; then
        if ! git -C "$src" rev-parse -q --verify "refs/tags/$to^{commit}" >/dev/null 2>&1; then
            fail "No release tag $to in the clone."
            return 1
        fi
        target="$to"
    else
        target="$(service_latest_release_tag "$src")"
        if [[ -z "$target" ]]; then
            fail "No release tag (X.Y.Z) found in the clone."
            return 1
        fi
    fi

    if [[ "$target" == "$cur" ]]; then
        success "Already up to date ($cur)."
        return 0
    fi
    if [[ "$check" == "1" ]]; then
        printf "  ${BOLD}Current${RESET}  %s\n  ${BOLD}Target${RESET}   %s\n" "$cur" "$target"
        info "An update is available: kronn service update"
        return 10
    fi
    printf "  ${BOLD}%s${RESET} -> ${BOLD}%s${RESET}\n" "$cur" "$target"

    if [[ -n "$(git -C "$src" status --porcelain --untracked-files=no 2>/dev/null)" ]]; then
        fail "The service clone $src has local modifications — refusing to touch it."
        return 1
    fi

    # Stage next to the running release; nothing live changes until the switch.
    if [[ -f "$home/releases/$target/VERSION" ]]; then
        info "Release $target is already staged — reusing it."
    else
        [[ -d "$HOME/.cargo/bin" ]] && PATH="$(path_append_missing "$PATH" "$HOME/.cargo/bin")"
        [[ -d "/opt/homebrew/bin" ]] && PATH="$(path_append_missing "$PATH" "/opt/homebrew/bin")"
        export PATH
        local missing
        missing="$(service_missing_tools)"
        if [[ -n "$missing" ]]; then
            fail "Missing prerequisite(s): $missing"
            return 1
        fi
        git -C "$src" checkout --quiet --detach "$target" || { fail "git checkout $target failed."; return 1; }
        service_build_from_source "$src" || return 1
        service_stage_release "$src" "$target" || return 1
        success "Staged release $target"
    fi
    # The changelog is read from the clone: make sure it is at the target.
    git -C "$src" checkout --quiet --detach "$target" 2>/dev/null || true

    if [[ "$force" == "1" ]]; then
        warn "--force: not waiting for in-flight work."
    elif ! _su_drain; then
        info "Aborted — nothing was changed (release $target stays staged)."
        return 1
    fi

    prev="$cur"
    old_previous="$(_su_read_previous)"
    backup_before="$(_su_mtime "$(service_db_backup)")"
    _su_write_state "$prev" "$target" "$backup_before" || { fail "Could not write $(_su_state_file)"; return 1; }

    info "Switching $prev -> $target and restarting..."
    service_switch_current "$target" || return 1
    if service_restart && service_wait_healthy "$(_su_backend_version "$target")" "${KRONN_SERVICE_HEALTH_TIMEOUT:-90}"; then
        :
    else
        fail "Release $target did not become healthy — rolling back to $prev."
        local migrated=0 now
        now="$(_su_mtime "$(service_db_backup)")"
        (( now > backup_before )) && migrated=1
        if _su_rollback "$prev" "$target" auto "$migrated"; then
            [[ -n "$old_previous" ]] && printf '%s\n' "$old_previous" > "$(_su_previous_file)"
            rm -f "$(_su_state_file)"
            fail "Update to $target failed; $prev is running again. Logs: kronn service logs"
        else
            fail "Update to $target failed AND the rollback did not complete. Inspect: kronn service status / logs"
        fi
        return 1
    fi

    printf '%s\n' "$prev" > "$(_su_previous_file)"
    success "Kronn $target is running (was $prev)."
    printf "  ${DIM}Undo with: kronn service rollback${RESET}\n"
    _su_prune_releases
    _su_print_changelog "$target" "$src"
    return 0
}

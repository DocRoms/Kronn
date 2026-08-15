#!/usr/bin/env bash
set -euo pipefail

ROOT="${KRONN_VERSION_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
failures=0

fail() {
    printf '✗ %s\n' "$1" >&2
    failures=$((failures + 1))
}

first_toml_version() {
    sed -n 's/^version = "\([^"]*\)"/\1/p' "$1" | head -n 1
}

json_version() {
    sed -n 's/^[[:space:]]*"version":[[:space:]]*"\([^"]*\)".*/\1/p' "$1" | head -n 1
}

lock_package_version() {
    awk -v package="$2" '
        $0 == "name = \"" package "\"" { found = 1; next }
        found && /^version = "/ {
            gsub(/^version = "|".*$/, "")
            print
            exit
        }
    ' "$1"
}

check_equal() {
    if [ "$2" != "$VERSION" ]; then
        fail "$1 is '$2' (expected '$VERSION')"
    fi
}

VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    printf '✗ VERSION is not a semantic version: %s\n' "$VERSION" >&2
    exit 1
fi

check_equal "backend/Cargo.toml" \
    "$(first_toml_version "$ROOT/backend/Cargo.toml")"
check_equal "desktop/src-tauri/Cargo.toml" \
    "$(first_toml_version "$ROOT/desktop/src-tauri/Cargo.toml")"
check_equal "frontend/package.json" \
    "$(json_version "$ROOT/frontend/package.json")"
check_equal "desktop/package.json" \
    "$(json_version "$ROOT/desktop/package.json")"
check_equal "desktop/src-tauri/tauri.conf.json" \
    "$(json_version "$ROOT/desktop/src-tauri/tauri.conf.json")"
check_equal "backend/Cargo.lock package kronn" \
    "$(lock_package_version "$ROOT/backend/Cargo.lock" "kronn")"
check_equal "desktop Cargo.lock package kronn" \
    "$(lock_package_version "$ROOT/desktop/src-tauri/Cargo.lock" "kronn")"
check_equal "desktop Cargo.lock package kronn-desktop" \
    "$(lock_package_version "$ROOT/desktop/src-tauri/Cargo.lock" "kronn-desktop")"

CHANGELOG_VERSION="$(
    sed -nE 's/^## \[([0-9]+\.[0-9]+\.[0-9]+)\]( - [0-9]{4}-[0-9]{2}-[0-9]{2})?$/\1/p' "$ROOT/CHANGELOG.md" |
        head -n 1
)"
check_equal "first CHANGELOG release" "$CHANGELOG_VERSION"

if ! grep -Fq "**Status: $VERSION (current release).**" "$ROOT/README.md"; then
    fail "README.md does not identify $VERSION as the current release"
fi
if ! grep -Fq "**Statut : $VERSION (version actuelle).**" "$ROOT/README.fr.md"; then
    fail "README.fr.md does not identify $VERSION as the current release"
fi
if ! grep -Fq "## What's new in $VERSION" "$ROOT/README.md"; then
    fail "README.md release heading is not $VERSION"
fi
if ! grep -Fq "## Nouveautés de la $VERSION" "$ROOT/README.fr.md"; then
    fail "README.fr.md release heading is not $VERSION"
fi

if ! grep -Fq "## Current release: $VERSION" "$ROOT/docs/index.md"; then
    fail "docs/index.md does not identify $VERSION as the current release"
fi

for readme in README.md README.fr.md; do
    if ! grep -Fq "git clone --branch $VERSION " "$ROOT/$readme"; then
        fail "$readme clone instructions do not use $VERSION"
    fi
    stale_clone="$(
        grep -Eo 'git clone --branch [0-9]+\.[0-9]+\.[0-9]+' "$ROOT/$readme" |
            sed 's/.*--branch //' |
            grep -vx "$VERSION" || true
    )"
    if [ -n "$stale_clone" ]; then
        fail "$readme still contains stale clone version(s): $stale_clone"
    fi
done

if ! grep -Fq "git clone --branch $VERSION " "$ROOT/docs/install.md"; then
    fail "docs/install.md clone instructions do not use $VERSION"
fi
stale_install="$(
    grep -Eo 'git clone --branch [0-9]+\.[0-9]+\.[0-9]+' "$ROOT/docs/install.md" |
        sed 's/.*--branch //' |
        grep -vx "$VERSION" || true
)"
if [ -n "$stale_install" ]; then
    fail "docs/install.md still contains stale clone version(s): $stale_install"
fi

for site in site/index.html site/en.html site/es.html; do
    if ! grep -Fq "v$VERSION" "$ROOT/$site"; then
        fail "$site does not mention v$VERSION"
    fi
    stale_site="$(
        grep -Eo 'v[0-9]+\.[0-9]+\.[0-9]+' "$ROOT/$site" |
            sort -u |
            grep -vx "v$VERSION" || true
    )"
    if [ -n "$stale_site" ]; then
        fail "$site still contains stale current-version marker(s): $stale_site"
    fi
    if ! grep -Eq "aria-label=\"[^\"]*Kronn $VERSION\"" "$ROOT/$site"; then
        fail "$site release-note label is not $VERSION"
    fi
    if ! grep -Fq "<strong>$VERSION</strong>" "$ROOT/$site"; then
        fail "$site release-note badge is not $VERSION"
    fi
done

if [ "$failures" -ne 0 ]; then
    printf '\nVersion consistency check failed (%s issue(s)).\n' "$failures" >&2
    exit 1
fi

printf '✓ Release version %s is synchronized everywhere.\n' "$VERSION"

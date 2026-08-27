#!/usr/bin/env bash
set -euo pipefail

ROOT="${KRONN_SITE_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
sites=(site/index.html site/en.html site/es.html)
failures=0

for site in "${sites[@]}"; do
    if grep -Eq 'body::after[[:space:]]*\{' "$ROOT/$site"; then
        printf '✗ %s still defines a body::after overlay\n' "$site" >&2
        failures=$((failures + 1))
    fi
done

if [ "$failures" -ne 0 ]; then
    exit 1
fi

printf '✓ Public-site locales contain no body::after scanline overlay.\n'

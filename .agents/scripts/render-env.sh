#!/bin/sh
set -eu

# Minimal renderer to generate .env.example from workflow files
# Extracts ${env:VAR} references and generates a deterministic .env.example

if [ "$#" -eq 0 ]; then
    echo "Usage: $0 <workflow.json...>" >&2
    exit 1
fi

# Extract all env vars, sort them uniquely, and format as VAR=
grep -hEo '\$\{env:[a-zA-Z0-9_]+\}' "$@" | \
    sed -E 's/\$\{env:([a-zA-Z0-9_]+)\}/\1/' | \
    sort -u | \
    awk '{print $1 "="}' > .env.example

echo "Generated .env.example with $(wc -l < .env.example | tr -d ' ') variables."

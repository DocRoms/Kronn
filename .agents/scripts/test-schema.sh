#!/bin/sh
set -eu

SCHEMA=".agents/schema/workflow.v1.schema.json"
FIXTURES_DIR=".agents/schema/fixtures"

echo "Running schema tests using ajv-cli..."

if ! command -v npx >/dev/null 2>&1; then
    echo "npx not found, cannot run schema tests." >&2
    exit 1
fi

# Valid fixtures
for f in "$FIXTURES_DIR"/valid-*.json; do
    if [ -f "$f" ]; then
        echo "Testing valid fixture: $f"
        npx --yes ajv-cli validate -s "$SCHEMA" -d "$f"
    fi
done

# Invalid fixtures
for f in "$FIXTURES_DIR"/invalid-*.json; do
    if [ -f "$f" ]; then
        echo "Testing invalid fixture: $f"
        if npx --yes ajv-cli validate -s "$SCHEMA" -d "$f" >/dev/null 2>&1; then
            echo "Error: Invalid fixture $f passed validation!" >&2
            exit 1
        else
            echo "Invalid fixture $f correctly rejected."
        fi
    fi
done

echo "All schema tests passed."

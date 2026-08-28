#!/bin/sh
set -eu

echo "Testing render-env.sh..."
FIXTURE=".agents/schema/fixtures/valid-all-types.json"
OUTPUT=$(python3 .agents/scripts/render-env.sh "$FIXTURE")
EXPECTED="HOST_SECRET="

if [ "$OUTPUT" != "$EXPECTED" ]; then
    echo "Error: render-env.sh output mismatch." >&2
    echo "Expected: $EXPECTED" >&2
    echo "Got: $OUTPUT" >&2
    exit 1
fi
echo "render-env.sh passed."

echo "Testing bootstrap.sh..."
# Run bootstrap in a temporary directory to avoid polluting the workspace
TMP_DIR=$(mktemp -d)
cp "$FIXTURE" "$TMP_DIR/workflow.json"
sh .agents/scripts/bootstrap.sh "$TMP_DIR/workflow.json"

if [ ! -f "$TMP_DIR/.env.example" ]; then
    echo "Error: bootstrap.sh did not generate .env.example" >&2
    exit 1
fi

if grep -q "HOST_SECRET=" "$TMP_DIR/.env.example"; then
    echo "bootstrap.sh passed."
else
    echo "Error: .env.example content mismatch." >&2
    cat "$TMP_DIR/.env.example" >&2
    exit 1
fi

rm -rf "$TMP_DIR"
echo "All script tests passed."

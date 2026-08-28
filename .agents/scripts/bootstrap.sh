#!/bin/sh
set -eu

# Bootstrap meta-skill for Kronn portable workflows
# Validates dependencies and prepares the environment

WORKFLOW_FILE="${1:-}"

if [ -z "$WORKFLOW_FILE" ]; then
    echo "Usage: $0 <workflow.json>" >&2
    exit 1
fi

if [ ! -f "$WORKFLOW_FILE" ]; then
    echo "Error: Workflow file '$WORKFLOW_FILE' not found." >&2
    exit 1
fi

echo "Bootstrapping workflow: $WORKFLOW_FILE"

# Generate .env.example
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
"$SCRIPT_DIR/render-env.sh" "$WORKFLOW_FILE"

# Check if .env exists, if not copy .env.example
if [ ! -f .env ] && [ -f .env.example ]; then
    echo "No .env found. Copying .env.example to .env. Please fill in the values."
    cp .env.example .env
fi

# Extract and check requires (using grep/sed for portability without jq)
# This is a minimal check. In a real scenario, jq would be better, but we want portability.
REQUIRES=$(grep -Eo '"requires"\s*:\s*\[(.*)\]' "$WORKFLOW_FILE" | sed -E 's/.*\[(.*)\].*/\1/' | tr -d '"' | tr ',' ' ' || true)

for req in $REQUIRES; do
    if ! command -v "$req" >/dev/null 2>&1; then
        echo "Warning: Required command '$req' is not installed or not in PATH." >&2
    else
        echo "Requirement satisfied: $req"
    fi
done

echo "Bootstrap complete."

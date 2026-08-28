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
WORKFLOW_DIR="$(cd "$(dirname "$WORKFLOW_FILE")" && pwd)"
ENV_EXAMPLE="$WORKFLOW_DIR/.env.example"

python3 "$SCRIPT_DIR/render-env.sh" "$WORKFLOW_FILE" > "$ENV_EXAMPLE"
echo "Generated $ENV_EXAMPLE"
echo "Please review $ENV_EXAMPLE and create your own .env file if needed."

# Extract and check requires (using python3 for portability and correctness)
REQUIRES=$(python3 -c '
import sys, json
try:
    with open(sys.argv[1]) as f:
        reqs = json.load(f).get("requires", [])
        if isinstance(reqs, list):
            print(" ".join(reqs))
except:
    pass
' "$WORKFLOW_FILE")

for req in $REQUIRES; do
    if ! command -v "$req" >/dev/null 2>&1; then
        echo "Warning: Required command '$req' is not installed or not in PATH." >&2
    else
        echo "Requirement satisfied: $req"
    fi
done

echo "Bootstrap complete."

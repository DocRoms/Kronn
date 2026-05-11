#!/usr/bin/env bash
# scripts/seed-demo-fixtures.sh
#
# Spawn a sandboxed Kronn instance with curated demo data (projects,
# Quick Prompts, MCP configs) for README screenshots, marketing GIFs,
# or onboarding tutorials. Reproducible, no real user data ever touched.
#
# # Why this script exists
#
# Screenshots in the README/website should NOT show the maintainer's
# real project names, real Jira tickets, or real MCP secrets. This
# script materializes a parallel "demo universe" :
#   - `acme-blog`, `demo-monorepo`, `sample-rust-cli` projects
#   - 4 Quick Prompts with marketing-friendly names + descriptions
#   - 1 active batch run (for the batch-running screenshot)
#   - Setup wizard already completed
# in a tmpdir + a non-default port, so it CO-EXISTS with the
# maintainer's prod Kronn on the standard port.
#
# # How to use
#
#   1. Make sure your real Kronn is on :3140 (default), this sandbox
#      uses :3145. Or stop your real Kronn; both will work.
#   2. Run this script. It prints a frontend command to launch.
#   3. Take screenshots. Trash the tmpdir when done (script tells you how).
#
# # Requirements
#
#   - Kronn backend built (`cargo build --release --bin kronn` OR debug)
#   - `curl` + `git` on PATH (no `jq`, no Python; pure Bash + curl)
#
# # Env vars (all optional, sensible defaults)
#
#   KRONN_SANDBOX_PORT=3145          Where the sandbox backend listens
#   KRONN_SANDBOX_DATA=/tmp/kronn-demo-XXXXXX   Data dir (mktemp by default)
#   KRONN_SANDBOX_REPOS=/tmp/kronn-demo-repos-XXXXXX  Demo repo dir
#   KRONN_BINARY=./backend/target/release/kronn   Path to the backend binary
#
# # Exit codes
#
#   0  success: sandbox running, instructions printed
#   1  setup failure (curl missing, backend not built, port in use, …)

set -euo pipefail

# ── Defaults ───────────────────────────────────────────────────────────
PORT="${KRONN_SANDBOX_PORT:-3145}"
DATA_DIR="${KRONN_SANDBOX_DATA:-$(mktemp -d -t kronn-demo-data.XXXXXX)}"
REPOS_DIR="${KRONN_SANDBOX_REPOS:-$(mktemp -d -t kronn-demo-repos.XXXXXX)}"
BINARY="${KRONN_BINARY:-./backend/target/release/kronn}"
if [ ! -x "$BINARY" ]; then
  BINARY="./backend/target/debug/kronn"
fi
API="http://localhost:${PORT}/api"

# ── Pre-flight ─────────────────────────────────────────────────────────
command -v curl >/dev/null || { echo "✗ curl is required"; exit 1; }
command -v git  >/dev/null || { echo "✗ git is required"; exit 1; }
# Zero external JSON tooling Bash + curl only. Demo data is small,
# hand-typed JSON literals are easier to audit than jq pipelines.
if [ ! -x "$BINARY" ]; then
  echo "✗ Kronn binary not found. Run:  cargo build --release --bin kronn  (in backend/)"
  echo "  Or set KRONN_BINARY=/path/to/kronn"
  exit 1
fi
if curl -fsS "$API/setup/status" >/dev/null 2>&1; then
  echo "✗ Port $PORT already serving HTTP. Refusing to clobber."
  echo "  Pick another with KRONN_SANDBOX_PORT=3146 ./scripts/seed-demo-fixtures.sh"
  exit 1
fi

echo "▸ Sandbox config:"
echo "    PORT      = $PORT"
echo "    DATA_DIR  = $DATA_DIR"
echo "    REPOS_DIR = $REPOS_DIR"
echo "    BINARY    = $BINARY"
echo

# ── Pre-seed config.toml so the backend boots on $PORT ──────────────
# The backend reads port from `config.toml[server].port` only -
# there is no `KRONN_PORT` env var override. Several fields (notably
# `[scan].ignore`) are required at deserialization time, so we mirror
# what `default_config()` produces in core/config.rs with just the
# port + scan paths overridden for the sandbox.
echo "▸ Pre-seeding config.toml with port $PORT…"
cat > "$DATA_DIR/config.toml" <<EOF
[server]
host = "127.0.0.1"
port = $PORT
auth_enabled = false
auth_strict_localhost = false
max_concurrent_agents = 5
agent_stall_timeout_min = 5
global_context_mode = "always"
debug_mode = false

[tokens]
keys = []
disabled_overrides = []

[scan]
paths = ["$REPOS_DIR"]
ignore = ["node_modules", ".git", "target", "vendor", "dist", "build", ".cache"]
scan_depth = 4

# `agents.claude_code` and `agents.codex` are required at the
# AgentsConfig level (no #[serde(default)] on them). Empty subsections
# satisfy the schema since AgentConfig derives Default.
[agents.claude_code]
[agents.codex]
EOF

# ── Spawn backend in background ───────────────────────────────────────
echo "▸ Starting backend…"
KRONN_DATA_DIR="$DATA_DIR" \
  "$BINARY" > /tmp/kronn-demo-backend.log 2>&1 &
BACKEND_PID=$!
echo "$BACKEND_PID" > /tmp/kronn-demo-backend.pid

# Wait for /api/setup/status to respond (max 30s).
for i in $(seq 1 30); do
  if curl -fsS "$API/setup/status" >/dev/null 2>&1; then
    echo "  backend ready after ${i}s (pid $BACKEND_PID)"
    break
  fi
  sleep 1
done
if ! curl -fsS "$API/setup/status" >/dev/null 2>&1; then
  echo "✗ Backend did not start. Tail of /tmp/kronn-demo-backend.log:"
  tail -30 /tmp/kronn-demo-backend.log
  kill "$BACKEND_PID" 2>/dev/null || true
  exit 1
fi

# ── Finalize setup ────────────────────────────────────────────────────
# The pre-seeded config.toml already has `scan.paths` set, which makes
# `setup_status` return Complete on the fast path. We still call
# /setup/complete explicitly to force a full config save with all
# default fields materialised keeps the runtime in a predictable
# state for the screenshot session.
echo
echo "▸ Finalizing setup…"
curl -fsS -X POST "$API/setup/complete" >/dev/null

# ── Create 3 demo projects ─────────────────────────────────────────────
echo "▸ Creating 3 demo project repos under $REPOS_DIR…"
DEMO_PROJECTS=(
  "acme-blog|A fictional company's blog backend (Node.js + Postgres)"
  "demo-monorepo|Polyglot monorepo: Rust backend + React frontend"
  "sample-rust-cli|Small command-line tool written in Rust"
)
for entry in "${DEMO_PROJECTS[@]}"; do
  name="${entry%%|*}"
  desc="${entry##*|}"
  path="$REPOS_DIR/$name"
  mkdir -p "$path"
  ( cd "$path" && git init -q . && \
    git config user.email "demo@kronn.local" && \
    git config user.name "Kronn Demo" && \
    echo "# $name" > README.md && echo "$desc" >> README.md && \
    git add . && git commit -q -m "init" )
  # Inline JSON `path` is a /tmp/* absolute path we control, no
  # special characters to escape; same for `name` (kebab-case slug).
  curl -fsS -X POST -H "Content-Type: application/json" \
    -d "{\"path\":\"$path\",\"name\":\"$name\"}" \
    "$API/projects/add-folder" >/dev/null
  echo "  ✓ $name"
done

# ── Seed 4 Quick Prompts ───────────────────────────────────────────────
# Names + descriptions chosen for marketing-friendly screenshots -
# nothing references real customer/issue tracker data. Keep them
# generic and obviously demonstrative.
echo
echo "▸ Seeding 4 demo Quick Prompts…"

# Each QP is a single-line JSON literal POSTed individually no jq
# pipeline needed. POST_QP "<display name>" '<json>' for clarity.
post_qp() {
  local label="$1"; local body="$2"
  curl -fsS -X POST -H "Content-Type: application/json" -d "$body" \
    "$API/quick-prompts" >/dev/null && echo "  ✓ $label"
}

post_qp "Analyse Jira ticket" '{
  "name":"Analyse Jira ticket",
  "icon":"🎯",
  "prompt_template":"Analyse Jira ticket {{ticket}}. Identify the user story, list any technical risks, propose an implementation plan in 3-5 steps.",
  "variables":[{"name":"ticket","label":"Ticket key","placeholder":"PROJ-123","required":true}],
  "agent":"ClaudeCode","tier":"default","skill_ids":[],
  "description":"One-shot ticket triage drop a Jira key, get a structured plan back."
}'

post_qp "Generate changelog" '{
  "name":"Generate changelog",
  "icon":"📝",
  "prompt_template":"Generate a CHANGELOG.md entry for the commits between {{from_tag}} and HEAD. Group by Added/Changed/Fixed/Removed.",
  "variables":[{"name":"from_tag","label":"Previous tag","placeholder":"v1.2.0","required":true}],
  "agent":"ClaudeCode","tier":"default","skill_ids":[],
  "description":"Auto-format CHANGELOG sections from git history."
}'

post_qp "Audit module security" '{
  "name":"Audit module security",
  "icon":"🛡️",
  "prompt_template":"Audit module {{module}} for security risks: SQL injection, XSS, SSRF, secrets in code, weak crypto. Return findings as a numbered list with severity.",
  "variables":[{"name":"module","label":"Module path","placeholder":"src/api/auth","required":true}],
  "agent":"Codex","tier":"reasoning","skill_ids":[],
  "description":"Security pass over a single module. Pair with Compare-agents for second opinion."
}'

post_qp "Refactor for testability" '{
  "name":"Refactor for testability",
  "icon":"🔧",
  "prompt_template":"Refactor {{function}} to be unit-testable: extract pure functions, remove I/O from the core logic, document the contract.",
  "variables":[{"name":"function","label":"Function name","placeholder":"processOrder","required":true}],
  "agent":"ClaudeCode","tier":"default","skill_ids":[],
  "description":"Split a tangled function into testable bits."
}'

# ── Print next steps ───────────────────────────────────────────────────
cat <<EOF

✓ Sandbox ready

Backend running on  http://localhost:$PORT
Frontend launch (from frontend/ dir, on port 5174 to avoid colliding
with a default Vite session pointed at your prod backend):

    cd frontend && KRONN_BACKEND_URL=http://localhost:$PORT VITE_DEV_PORT=5174 pnpm dev

(then open http://localhost:5174 in your browser)

Take your screenshots, then teardown with:

    kill \$(cat /tmp/kronn-demo-backend.pid) && \\
    rm -rf "$DATA_DIR" "$REPOS_DIR" /tmp/kronn-demo-backend.{log,pid}

EOF

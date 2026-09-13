#!/usr/bin/env bash
#
# Tests for scripts/e2e-sandbox-backend.sh.
#
# A script whose whole job is "never touch the developer's instance" has to be
# tested without one. Every case here runs against a FAKE backend — a few lines
# of Python that read the port out of the config the script wrote — so nothing
# in this file can reach a real Kronn, a real database, or port 3140.
#
# Run:  scripts/tests/e2e-sandbox-backend.test.sh

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT="$ROOT/scripts/e2e-sandbox-backend.sh"
WORK="$(mktemp -d)"
PASS=0
FAIL=0

cleanup() {
    # Kill anything this file started, by pid, and nothing else.
    for pidfile in "$WORK"/*.pid; do
        [[ -f "$pidfile" ]] && kill "$(cat "$pidfile")" 2>/dev/null
    done
    rm -rf "$WORK"
}
trap cleanup EXIT

ok()   { PASS=$((PASS + 1)); echo "  ok   — $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  FAIL — $1"; [[ -n "${2:-}" ]] && echo "         $2"; }

# A port nobody is on. Asking the kernel beats picking a number and hoping.
free_port() {
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

# ── The fakes ───────────────────────────────────────────────────────────────

make_fake() {
    # $1: path, $2: one of healthy | silent | exits
    local path="$1" mode="$2"
    cat > "$path" <<PY
#!/usr/bin/env python3
import os, re, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

MODE = "$mode"
if MODE == "exits":
    sys.stderr.write("fake backend refuses to start\\n")
    sys.exit(3)

# Record the environment the script handed us, so the isolation of host
# config writes is asserted rather than described in a comment.
with open(os.path.join(os.environ["KRONN_DATA_DIR"], "env-seen.txt"), "w") as fh:
    fh.write(os.environ.get("KRONN_HOST_HOME", "<unset>"))

# The port is whatever the script wrote into the config it owns.
config = open(os.path.join(os.environ["KRONN_DATA_DIR"], "config.toml")).read()
port = int(re.search(r"^port = (\\d+)\$", config, re.M).group(1))

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'{"ok":true}' if MODE == "healthy" else b'{"still":"booting"}'
        self.send_response(200)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, *args):
        pass

HTTPServer(("127.0.0.1", port), Handler).serve_forever()
PY
    chmod +x "$path"
}

refuses() {
    local pattern="$1" why="$2"; shift 2
    local out status
    out="$("$@" 2>&1)"
    status=$?
    if [[ $status -eq 0 ]]; then
        bad "$why" "it accepted, and printed: $out"
    elif ! grep -q "$pattern" <<<"$out"; then
        # Refused for a reason that is not the one under test. This is the
        # trap: on a machine where the developer's backend is running, port
        # 3140 is refused as "already served" whatever the port guard does.
        bad "$why" "refused for another reason: $out"
    else
        ok "$why"
    fi
}

echo "e2e-sandbox-backend.sh"

# ── Refusals that cost nothing, because nothing has started ─────────────────

FAKE="$WORK/fake-healthy"
make_fake "$FAKE" healthy
export KRONN_E2E_BINARY="$FAKE"

refuses "must be a number" "refuses a port that is not a number" \
    "$SCRIPT" "$WORK/d1" "not-a-port"
refuses "out of range" "refuses a port below the unprivileged range" \
    "$SCRIPT" "$WORK/d2" 80
refuses "out of range" "refuses a port above 65535" \
    "$SCRIPT" "$WORK/d3" 70000
refuses "developer instance" "refuses the developer backend port" \
    "$SCRIPT" "$WORK/d4" 3140
refuses "developer instance" "refuses the developer Vite port" \
    "$SCRIPT" "$WORK/d5" 5173

mkdir -p "$WORK/already-there"
refuses "already exists" "refuses a data directory it does not own" \
    "$SCRIPT" "$WORK/already-there" "$(free_port)"

KRONN_E2E_BINARY="$WORK/no-such-binary" \
    refuses "no backend binary" "refuses a missing binary" \
        "$SCRIPT" "$WORK/d6" "$(free_port)"

# A port somebody else is serving. This is the case that matters most: a stale
# sandbox holding the port answers everything, and taking it as ours is how a
# run reads a database nobody is serving.
BUSY_PORT="$(free_port)"
python3 -c "
import socket, time
s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(('127.0.0.1', $BUSY_PORT)); s.listen(1); time.sleep(120)
" & echo $! > "$WORK/busy.pid"
sleep 1
refuses "already served" "refuses a port somebody else is already serving" \
    "$SCRIPT" "$WORK/d7" "$BUSY_PORT"

# ── The happy path ──────────────────────────────────────────────────────────

PORT="$(free_port)"
DIR="$WORK/live"
if PID="$("$SCRIPT" "$DIR" "$PORT" 2>"$WORK/live.err")"; then
    echo "$PID" > "$WORK/live.pid"
    [[ "$PID" =~ ^[0-9]+$ ]] \
        && ok "prints the pid and nothing else" \
        || bad "prints the pid and nothing else" "got: $PID"
    grep -q "^port = $PORT\$" "$DIR/config.toml" \
        && ok "writes the port it was given" \
        || bad "writes the port it was given"
    # Written, not produced: the config exists without the product having run
    # to make it, so it cannot have served a default port on the way.
    grep -q "Written by scripts/e2e-sandbox-backend.sh" "$DIR/config.toml" \
        && ok "writes the config itself rather than booting to get one" \
        || bad "writes the config itself rather than booting to get one"
    # `KRONN_DATA_DIR` alone does not isolate the MCP host sync: the backend
    # rewrites ~/.claude.json & friends at boot, dropping every Kronn-managed
    # entry before re-inserting from the database it was given. An empty
    # sandbox database therefore strips the developer's MCP configuration —
    # which is exactly what happened on 2026-09-13.
    SEEN="$(cat "$DIR/env-seen.txt" 2>/dev/null)"
    [[ "$SEEN" == "$DIR/host-home" ]] \
        && ok "points host config writes inside the sandbox" \
        || bad "points host config writes inside the sandbox" "KRONN_HOST_HOME=$SEEN"
    [[ -d "$DIR/host-home" ]] \
        && ok "creates the host-home it points at" \
        || bad "creates the host-home it points at"

    LISTENER="$(lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null | head -1)"
    [[ "$LISTENER" == "$PID" ]] \
        && ok "the pid it returns is the one holding the port" \
        || bad "the pid it returns is the one holding the port" "listener=$LISTENER pid=$PID"
    kill "$PID" 2>/dev/null
else
    bad "starts a healthy fake backend" "$(cat "$WORK/live.err")"
fi

# ── Failures after the spawn must not leave a process behind ────────────────

make_fake "$WORK/fake-silent" silent
PORT="$(free_port)"
KRONN_E2E_BINARY="$WORK/fake-silent" KRONN_E2E_HEALTH_TRIES=3 \
    "$SCRIPT" "$WORK/silent" "$PORT" >"$WORK/silent.out" 2>"$WORK/silent.err" &
script_pid=$!
# Catch the process the script spawned WHILE it is still up, so the assertion
# below names a pid rather than trusting an empty port.
spawned=""
for _ in $(seq 1 40); do
    spawned="$(lsof -nP -iTCP:"$PORT" -sTCP:LISTEN -t 2>/dev/null | head -1)"
    [[ -n "$spawned" ]] && break
    sleep 0.2
done
wait "$script_pid"
[[ -n "$spawned" ]] \
    && ok "the fake did start and hold the port" \
    || bad "the fake did start and hold the port" "never saw a listener on $PORT"
if [[ -s "$WORK/silent.out" ]]; then
    bad "refuses a backend that listens but is never healthy" "it returned a pid"
else
    ok "refuses a backend that listens but is never healthy"
fi
grep -q "never answered a healthy" "$WORK/silent.err" \
    && ok "says the health check is what failed" \
    || bad "says the health check is what failed" "$(cat "$WORK/silent.err")"
sleep 1
if [[ -n "$spawned" ]] && kill -0 "$spawned" 2>/dev/null; then
    kill "$spawned" 2>/dev/null
    bad "kills what it started when the health check never passes" "pid $spawned survived"
else
    ok "kills what it started when the health check never passes"
fi

make_fake "$WORK/fake-exits" exits
PORT="$(free_port)"
KRONN_E2E_BINARY="$WORK/fake-exits" "$SCRIPT" "$WORK/exits" "$PORT" \
    >"$WORK/exits.out" 2>"$WORK/exits.err"
if [[ -s "$WORK/exits.out" ]]; then
    bad "refuses a backend that exits during startup" "it returned a pid"
else
    ok "refuses a backend that exits during startup"
fi
grep -q "exited during startup" "$WORK/exits.err" \
    && ok "says why, with the backend's own last line" \
    || bad "says why, with the backend's own last line" "$(cat "$WORK/exits.err")"

echo
echo "$PASS passed, $FAIL failed"
[[ "$FAIL" -eq 0 ]]

#!/usr/bin/env bash
# Runs the real .docker/nginx.conf in nginx:alpine, with stub backend and
# frontend, and checks the app document's CSP. Needs Docker; not run in CI.
# Usage: backend/scripts/ci/gateway_frame_src_smoke.sh [port]
set -u
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PORT="${1:-38140}"
NET=kronn-frame-src-smoke; P=kronn-frame-src-smoke
DIR="$(mktemp -d)"
fail=0
check() { if eval "$2"; then echo "PASS $1"; else echo "FAIL $1"; fail=1; fi; }
cleanup() { docker rm -f $P-gw $P-be $P-fe >/dev/null 2>&1; docker network rm $NET >/dev/null 2>&1; }
trap 'cleanup; rm -rf "$DIR"' EXIT
cleanup

echo '<!doctype html><title>app</title>' > "$DIR/index.html"
cat > "$DIR/frontend.conf" <<'EOF'
server { listen 80; root /usr/share/nginx/html;
  location = /index.html { add_header Cache-Control "no-cache, must-revalidate"; }
  location / { try_files $uri $uri/ /index.html; } }
EOF
backend_conf() { # $1: X-Kronn-Frame-Src value
  cat > "$DIR/backend.conf" <<EOF
server { listen 3140;
  location = /api/embed-origins/frame-src { add_header X-Kronn-Frame-Src "$1"; return 204; }
  location /api/ { default_type application/json; return 200 '{}'; } }
EOF
}
backend_conf "'self' https://suno.com https://player.example.com:8443"

docker network create $NET >/dev/null
docker run -d --name $P-fe --network $NET --network-alias frontend \
  -v "$DIR/index.html:/usr/share/nginx/html/index.html:ro" \
  -v "$DIR/frontend.conf:/etc/nginx/conf.d/default.conf:ro" nginx:alpine >/dev/null
docker run -d --name $P-be --network $NET --network-alias backend \
  -v "$DIR:/cfg:ro" nginx:alpine \
  sh -c 'cp /cfg/backend.conf /etc/nginx/conf.d/default.conf && exec nginx -g "daemon off;"' >/dev/null
docker run -d --name $P-gw --network $NET -p "127.0.0.1:$PORT:80" \
  -v "$ROOT/.docker/nginx.conf:/etc/nginx/conf.d/default.conf:ro" nginx:alpine >/dev/null
sleep 2

headers() { curl -s -D - -o /dev/null "$@" | tr -d '\r'; }
csp() { grep -i '^content-security-policy:'; }
doc=$(headers "http://127.0.0.1:$PORT/discussions/x")
echo "$doc" | csp
check "one CSP header on the document" '[ "$(echo "$doc" | csp | wc -l | tr -d " ")" = 1 ]'
check "frame-src is exactly self + the allowed sites" \
  'echo "$doc" | csp | grep -q "; frame-src '"'"'self'"'"' https://suno.com https://player.example.com:8443;\$"'
for h in X-Frame-Options X-Content-Type-Options X-XSS-Protection Referrer-Policy; do
  check "$h kept on the document" 'echo "$doc" | grep -qi "^$h:"'
done
check "no validators on the document" '! echo "$doc" | grep -qi "^etag:\|^last-modified:"'
cond=$(headers -H 'If-None-Match: "x"' -H 'If-Modified-Since: Thu, 01 Jan 2099 00:00:00 GMT' "http://127.0.0.1:$PORT/")
check "a conditional reload gets a full 200" 'echo "$cond" | head -1 | grep -q " 200"'
check "/api keeps frame-src self" \
  'headers "http://127.0.0.1:$PORT/api/x" | csp | grep -q "frame-src '"'"'self'"'"';\$"'
check "assets keep frame-src self" \
  'headers "http://127.0.0.1:$PORT/assets/none.js" | csp | grep -q "frame-src '"'"'self'"'"';\$"'
check "the lookup location is internal" \
  '[ "$(curl -s -o /dev/null -w "%{http_code}" "http://127.0.0.1:$PORT/_kronn/frame-src")" != 204 ]'

backend_conf "'self' https://other.example"
docker exec $P-be sh -c 'cp /cfg/backend.conf /etc/nginx/conf.d/default.conf && nginx -s reload' 2>/dev/null
sleep 1
check "a list change applies at the next load, no gateway reload" \
  'headers "http://127.0.0.1:$PORT/" | csp | grep -q "frame-src '"'"'self'"'"' https://other.example;\$"'

docker stop $P-be >/dev/null
down=$(headers "http://127.0.0.1:$PORT/")
check "backend down: document still served, frame-src self" \
  'echo "$down" | head -1 | grep -q " 200" && echo "$down" | csp | grep -q "frame-src '"'"'self'"'"';\$"'

exit $fail

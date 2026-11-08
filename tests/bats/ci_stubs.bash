# Stubs for the CI scripts that talk to GitHub through `gh` and wait with `sleep`.
#
# `gh api [-X <method>] <url> [--jq <filter>]` serves $FIXTURES/<url path with
# "/" as "_">.json through the caller's own --jq filter, and logs every call in
# $FIXTURES/calls.log. A POST answers 0, or 1 when its URL contains one of the
# space-separated words of $POST_REFUSED (a fork's read-only token).
#
# `sleep` returns at once, logs "sleep <n>", and runs $FIXTURES/on_sleep when it
# exists: a test sets it with `on_sleep <shell command>` to change the world
# while the script waits. $FIXTURES/sleeps counts the calls.

install_ci_stubs() {
    TEST_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/kronn-ci-XXXXXX")"
    mkdir -p "$TEST_TMPDIR/bin" "$TEST_TMPDIR/fx"
    cat > "$TEST_TMPDIR/bin/gh" <<'GH'
#!/usr/bin/env bash
shift
method=GET url="" filter="."
while [ $# -gt 0 ]; do
    case "$1" in
        -X) method="$2"; shift 2 ;;
        --jq) filter="$2"; shift 2 ;;
        *) url="$1"; shift ;;
    esac
done
echo "$method $url" >> "$FIXTURES/calls.log"
if [ "$method" = POST ]; then
    for refused in $POST_REFUSED; do
        case "$url" in *"$refused"*) exit 1 ;; esac
    done
    exit 0
fi
key="$(printf '%s' "$url" | sed -E 's#^repos/[^/]+/[^/]+/##; s#\?.*##; s#/#_#g')"
jq -r "$filter" "$FIXTURES/$key.json"
GH
    cat > "$TEST_TMPDIR/bin/sleep" <<'SLEEP'
#!/usr/bin/env bash
echo "sleep $1" >> "$FIXTURES/calls.log"
echo >> "$FIXTURES/sleeps"
[ ! -f "$FIXTURES/on_sleep" ] || . "$FIXTURES/on_sleep"
SLEEP
    chmod +x "$TEST_TMPDIR/bin/gh" "$TEST_TMPDIR/bin/sleep"
    export PATH="$TEST_TMPDIR/bin:$PATH" FIXTURES="$TEST_TMPDIR/fx"
    export POST_REFUSED=""
}

# on_sleep <shell command>: run it on every `sleep`. $PPID is the script waiting.
on_sleep() {
    printf '%s\n' "$1" > "$FIXTURES/on_sleep"
}

# Number of sleeps so far.
sleeps() {
    if [ -f "$FIXTURES/sleeps" ]; then wc -l < "$FIXTURES/sleeps" | tr -d ' '; else echo 0; fi
}

# True when the stubbed `gh` was asked to POST <url> (e.g. repos/o/r/actions/runs/1/cancel).
posted() {
    grep -qxF "POST $1" "$FIXTURES/calls.log" 2>/dev/null
}

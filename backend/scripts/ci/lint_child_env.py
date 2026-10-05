#!/usr/bin/env python3
"""CI gate (KT-1006, layer A): every process Kronn starts gets a built
environment, never the backend's (which holds Kronn's admin token, its key
and provider keys).

Every `async_cmd(...)` / `sync_cmd(...)` call in production code must, in the
same function:
  - start `git` with a literal "git" (core::cmd gives git its own policy), or
  - apply `child_env::` (isolate, reset/seal, isolate_with_github), or
  - be one of the declared exceptions below (design note §9).
`tool_cmd` / `sync_tool_cmd` are isolated by construction and not listed.
Test code (`#[cfg(test)]` / `#[test]` items, `*_test.rs`, `*_tests.rs`) is
exempt. A stale exception (no call site left) fails too.
Run from `backend/`: `python3 scripts/ci/lint_child_env.py`."""
import importlib.util
import pathlib
import re
import sys

_spec = importlib.util.spec_from_file_location(
    "lint_wcu", pathlib.Path(__file__).parent / "lint_with_conn_unwrap.py")
_wcu = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_wcu)
sanitize = _wcu._sanitize

# (file under src/, enclosing function) -> why it keeps the backend's
# environment. Mirrors docs/design/agent-secret-boundary.md §9.
EXCEPTIONS = {
    ("core/docs_sidecar.rs", "start"): "document sidecar",
    ("core/model_catalog/claude_discovery.rs", "discover"): "model discovery",
    ("core/model_catalog/codex_discovery.rs", "discover"): "model discovery",
    ("core/versions.rs", "probe_installed_version"): "version discovery",
    ("agents/mod.rs", "get_version_from"): "version discovery",
    ("agents/mod.rs", "probe_runtime"): "version discovery (npx runtime probe)",
    ("api/mcps.rs", "probe_mcp_stdio_with_timeout"): "MCP probe",
}

# Files that define the helpers themselves.
SKIPPED_FILES = {"core/cmd.rs"}

CALL = re.compile(r"\b(async_cmd|sync_cmd)\s*\(")
GIT_LITERAL = re.compile(r'(?:async_cmd|sync_cmd)\s*\(\s*"git"\s*\)')
FN = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
TEST_ATTR = re.compile(r"#\[\s*(?:cfg\s*\(\s*(?:all\s*\(\s*)?test\b|(?:tokio::)?test\b)")


def _block_end(text, open_brace):
    """Index just past the brace matching `text[open_brace]`."""
    depth = 0
    for k in range(open_brace, len(text)):
        if text[k] == "{":
            depth += 1
        elif text[k] == "}":
            depth -= 1
            if depth == 0:
                return k + 1
    raise RuntimeError(f"unbalanced braces from offset {open_brace}")


def _item_end(text, start):
    """End of the item starting at `start`: its `;` or its balanced block."""
    brace = text.find("{", start)
    semi = text.find(";", start)
    if semi != -1 and (brace == -1 or semi < brace):
        return semi + 1
    return _block_end(text, brace)


def test_spans(text):
    """Spans of items under a test attribute (sanitized text)."""
    spans = []
    pos = 0
    while True:
        m = TEST_ATTR.search(text, pos)
        if not m:
            return spans
        attr_end = _block_end_bracket(text, m.start())
        spans.append((m.start(), _item_end(text, attr_end)))
        pos = spans[-1][1]


def _block_end_bracket(text, hash_pos):
    """Index just past the `]` closing the attribute at `hash_pos`."""
    depth = 0
    for k in range(hash_pos + 1, len(text)):
        if text[k] == "[":
            depth += 1
        elif text[k] == "]":
            depth -= 1
            if depth == 0:
                return k + 1
    raise RuntimeError(f"unclosed attribute at offset {hash_pos}")


def functions(text):
    """(name, body_start, body_end) for every fn with a body."""
    out = []
    for m in FN.finditer(text):
        brace = text.find("{", m.end())
        semi = text.find(";", m.end())
        if brace == -1 or (semi != -1 and semi < brace):
            continue  # trait method declaration or extern fn
        out.append((m.group(1), brace, _block_end(text, brace)))
    return out


def call_sites(raw, path="<mem>"):
    """[(line, function, verdict)] for every production spawn call site.

    verdict: "git", "isolated", or "bare" (needs an exception)."""
    text = sanitize(raw)
    skipped = test_spans(text)
    fns = functions(text)
    sites = []
    for m in CALL.finditer(text):
        at = m.start()
        if text[max(0, at - 3):at].endswith("fn "):
            continue
        if any(a <= at < b for a, b in skipped):
            continue
        enclosing = [f for f in fns if f[1] <= at < f[2]]
        line = text.count("\n", 0, at) + 1
        if not enclosing:
            raise RuntimeError(f"{path}:{line}: spawn call outside any function")
        name, start, end = min(enclosing, key=lambda f: f[2] - f[1])
        if GIT_LITERAL.match(raw, at):
            verdict = "git"
        elif "child_env::" in text[start:end]:
            verdict = "isolated"
        else:
            verdict = "bare"
        sites.append((line, name, verdict))
    return sites


def is_test_file(rel):
    name = pathlib.PurePosixPath(rel).name
    return name.endswith("_test.rs") or name.endswith("_tests.rs")


def check(root):
    """(violations, stale exceptions) for the tree under `root` (src/)."""
    violations, used = [], set()
    for f in sorted(root.rglob("*.rs")):
        rel = f.relative_to(root).as_posix()
        if rel in SKIPPED_FILES or is_test_file(rel):
            continue
        for line, name, verdict in call_sites(f.read_text(), rel):
            if verdict != "bare":
                continue
            if (rel, name) in EXCEPTIONS:
                used.add((rel, name))
            else:
                violations.append(f"src/{rel}:{line} in fn {name}")
    stale = sorted(f"src/{rel} fn {name}" for rel, name in set(EXCEPTIONS) - used)
    return violations, stale


def main():
    violations, stale = check(pathlib.Path("src"))
    if violations:
        print("::error::a child process keeps the backend's environment. Apply "
              "child_env::isolate (or use core::cmd::tool_cmd) in the same function, "
              "or declare the exception in design §9 and in this gate:")
        print("\n".join(violations))
    if stale:
        print("::error::declared spawn exceptions with no call site left (remove them here and in §9):")
        print("\n".join(stale))
    if violations or stale:
        sys.exit(1)
    print(f"OK: every spawn site builds its environment ({len(EXCEPTIONS)} declared exceptions)")


if __name__ == "__main__":
    main()

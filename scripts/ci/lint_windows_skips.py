#!/usr/bin/env python3
"""Every backend test skipped on Windows is `#[cfg(unix)]` with a one-line reason.

KT-1629. The nightly Windows lib run (ci-nightly-windows.yml) is only worth
reading if a skipped test is visible and justified, so:

  * a test gated to unix is `#[cfg(unix)]` (not `cfg(not(windows))`,
    `cfg_attr(windows, ignore)`, or an early `if cfg!(windows) { return }`);
  * the gate carries its reason: `#[cfg(unix)] // what Windows lacks` on the same
    line, or a `//` comment (not a `///` doc comment) on the line right above it.

A test is a function under `#[test]`, `#[tokio::test]` or `#[tokio::test(...)]`.
A `#[cfg(unix)]` on a module (`mod tests`) or an item that is not a test is the
module author's business and is not checked here.

Exit 0 when clean, 1 with one line per finding. `--list` prints every unix-only
test with its reason, the inventory the nightly job summarises.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCES = [ROOT / "backend" / "src", ROOT / "backend" / "tests"]

TEST_ATTR = re.compile(r"^\s*#\[(tokio::)?test\b")
UNIX_GATE = re.compile(r"^\s*#\[cfg\(unix\)\]\s*(//\s*(?P<reason>.*))?$")
OTHER_GATES = (
    (re.compile(r"#\[cfg\(not\(windows\)\)\]"), "use #[cfg(unix)], not cfg(not(windows))"),
    (re.compile(r"#\[cfg\(not\(target_os\s*=\s*\"windows\"\)\)\]"), "use #[cfg(unix)], not cfg(not(target_os))"),
    (re.compile(r"#\[cfg_attr\((windows|target_os\s*=\s*\"windows\"),\s*ignore"), "a test skipped on Windows is cfg(unix), not ignored"),
    (re.compile(r"#\[cfg_attr\(not\(unix\),\s*ignore"), "a test skipped on Windows is cfg(unix), not ignored"),
)
FN = re.compile(r"^\s*(pub(\([a-z]+\))?\s+)?(async\s+)?fn\s+(?P<name>\w+)")
EARLY_RETURN = re.compile(r"if\s+cfg!\(windows\)\s*\{\s*return|if\s+!cfg!\(unix\)\s*\{\s*return|if\s+cfg!\(target_os\s*=\s*\"windows\"\)\s*\{\s*return")


def rust_files():
    for base in SOURCES:
        yield from sorted(base.rglob("*.rs"))


def attributes_above(lines, index):
    """The attribute lines (and the comments between them) directly above `index`."""
    above = []
    i = index - 1
    while i >= 0 and (lines[i].strip().startswith("#[") or lines[i].strip().startswith("//") or lines[i].strip() == ""):
        if lines[i].strip() == "":
            break
        above.append((i, lines[i]))
        i -= 1
    return above


def scan(path):
    findings, unix_tests = [], []
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    try:
        rel = path.relative_to(ROOT)
    except ValueError:
        rel = path
    for index, line in enumerate(lines):
        fn = FN.match(line)
        if not fn:
            continue
        block = attributes_above(lines, index)
        if not any(TEST_ATTR.match(text) for _, text in block):
            continue
        name = fn.group("name")
        for at, text in block:
            for pattern, advice in OTHER_GATES:
                if pattern.search(text):
                    findings.append(f"{rel}:{at + 1}: {name}: {advice}")
            gate = UNIX_GATE.match(text)
            if gate:
                reason = gate.group("reason")
                if not reason and at > 0:
                    above = lines[at - 1].strip()
                    if above.startswith("//") and not above.startswith(("///", "//!")):
                        reason = above.lstrip("/").strip()
                if not reason:
                    findings.append(f"{rel}:{at + 1}: {name}: #[cfg(unix)] without a reason comment")
                unix_tests.append((rel, at + 1, name, reason or ""))
        indent = line[: len(line) - len(line.lstrip())]
        end = index + 1
        while end < len(lines) and lines[end].rstrip() != indent + "}":
            end += 1
        body = "\n".join(lines[index:end])
        if EARLY_RETURN.search(body):
            findings.append(f"{rel}:{index + 1}: {name}: skips itself on Windows at run time; gate it with #[cfg(unix)] and a reason")
    return findings, unix_tests


def main(argv):
    all_findings, all_unix = [], []
    for path in rust_files():
        findings, unix_tests = scan(path)
        all_findings += findings
        all_unix += unix_tests
    if "--list" in argv:
        for rel, line, name, reason in all_unix:
            print(f"{rel}:{line}: {name} — {reason}")
        print(f"{len(all_unix)} unix-only tests", file=sys.stderr)
        return 0
    for finding in all_findings:
        print(finding)
    if all_findings:
        print(f"{len(all_findings)} test(s) skipped on Windows without a cfg(unix) gate and a reason", file=sys.stderr)
        return 1
    print(f"ok: {len(all_unix)} unix-only tests, each with a reason")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

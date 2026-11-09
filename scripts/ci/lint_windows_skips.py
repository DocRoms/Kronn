#!/usr/bin/env python3
"""Every backend test skipped on Windows is `#[cfg(unix)]` with a concrete reason.

KT-1629. The nightly Windows lib run (ci-nightly-windows.yml) is only worth
reading if what it skips is visible and justified: a skip that is not must not
hide a real Windows bug. So:

  * a test, or a module holding tests, that does not build on Windows is gated
    with `#[cfg(unix)]` (or `all(unix, ...)`, `all(test, unix, ...)`). A
    `target_os = "linux"` / `"macos"` gate is accepted for what only that OS has.
    Every other way to skip on Windows (`not(windows)`, `target_family = "unix"`,
    `cfg_attr(windows, ignore)`, `if cfg!(windows) { return }`) is refused;
  * the gate carries its reason as a `//` comment on the SAME line:
    `#[cfg(unix)] // creates symlinks through std::os::unix`. When rustfmt moves
    a comment that runs past the line width, the plain `//` line directly below
    the gate is the reason; a comment above, or a `///` doc, never is;
  * the reason names a concrete POSIX cause (symlinks, chmod mode bits, fork and
    signals, unix sockets, a `#!/bin/sh` fixture, a program Windows lacks...).
    "unverified", "not yet run on Windows" and the like are refused: let the
    nightly run the test and decide. A gate kept for a known Windows defect says
    `see KT-<n>` and is accepted on that ground alone.

A "test" is a function under `#[test]`, `#[tokio::test]` or any `<path>::test`.
A module is checked when it holds a test (the inline body, or the file `mod x;`
or `#[path]` points to); a gate on a helper, a `use` or production code is not
checked: a helper only gates the tests that call it, and those carry a reason.

Exit 0 when clean, 1 with one line per finding. `--list` prints every unix-only
test with its reason, the inventory the nightly job summarises.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SOURCES = [ROOT / "backend" / "src", ROOT / "backend" / "tests"]

# What a reason must name to count as a cause, and what disqualifies it.
CONCRETE = re.compile(
    r"symlink|hard link|chmod|mode bit|inode|ctime|process group|signal|\bfork|setsid|getsid|"
    r"unix (domain )?socket|\bFIFO|/proc\b|libc|#!/|shell script|\bsh (script|stand)|"
    r"\bPOSIX\b|std::os::unix|OsStrExt|sandbox-exec|bwrap|bubblewrap|shebang|"
    r"windows (lacks|has no|cannot|does not|ignores)|\bwhich windows\b|unix-only|"
    r"cannot exec|as a program|\bunix\b.*\bonly\b|only\b.*\bunix\b|"
    r"cannot exist on windows|non-utf-?8|raw bytes|windows has neither|caffeinate|launchd|"
    r"only (on |in )?(macos|linux)",
    re.IGNORECASE,
)
VAGUE = re.compile(
    r"unverified|not (yet )?verified|gated before|not yet run|todo|fixme|for now|flaky|"
    r"fails? on windows|broken on windows|to be (checked|investigated)|tbd",
    re.IGNORECASE,
)
KNOWN_DEFECT = re.compile(r"\bsee KT-\d+\b")

TEST_ATTR = re.compile(r"^(?:\w+::)*test\b")
FN_HEAD = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+\"[^\"]*\"\s+)?fn\s+(?P<name>\w+)")
MOD_HEAD = re.compile(r"^(?:pub(?:\([^)]*\))?\s+)?mod\s+(?P<name>\w+)")
EARLY_RETURN = re.compile(
    r"if\s+(?:cfg!\(\s*windows\s*\)|cfg!\(\s*target_os\s*=\s*\"windows\"\s*\)|"
    r"!\s*cfg!\(\s*unix\s*\)|cfg!\(\s*not\(\s*unix\s*\)\s*\)|"
    r"!\s*cfg!\(\s*target_family\s*=\s*\"unix\"\s*\))\s*\{[^}]*?\breturn\b"
)

WINDOWS = {"unix": False, "windows": True, "target_os": "windows", "target_family": "windows", "target_env": "msvc"}
LINUX = {"unix": True, "windows": False, "target_os": "linux", "target_family": "unix", "target_env": "gnu"}
MACOS = {"unix": True, "windows": False, "target_os": "macos", "target_family": "unix", "target_env": ""}


def rust_files():
    for base in SOURCES:
        yield from sorted(base.rglob("*.rs"))


def mask(text):
    """`text` with comments and the inside of string and char literals blanked,
    newlines and offsets kept: structure can be read from it without being
    fooled by a brace or an attribute inside a string."""
    out = list(text)
    n = len(text)
    i = 0

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        two = text[i : i + 2]
        if two == "//":
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif two == "/*":
            depth, j = 1, i + 2
            while j < n and depth:
                if text[j : j + 2] == "/*":
                    depth, j = depth + 1, j + 2
                elif text[j : j + 2] == "*/":
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == "r" and re.match(r"r#*\"", text[i : i + 12]) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            hashes = len(re.match(r"r(#*)\"", text[i : i + 12]).group(1))
            close = "\"" + "#" * hashes
            j = text.find(close, i + 2 + hashes)
            j = n if j < 0 else j + len(close)
            blank(i + 1, j - 1)
            i = j
        elif c == "\"":
            j = i + 1
            while j < n and text[j] != "\"":
                j += 2 if text[j] == "\\" else 1
            blank(i + 1, min(j, n))
            i = j + 1
        elif c == "'":
            if text[i + 1 : i + 2] == "\\":
                j = text.find("'", i + 2)
                j = n if j < 0 else j
                blank(i + 1, j)
                i = j + 1
            elif text[i + 2 : i + 3] == "'":
                blank(i + 1, i + 2)
                i += 3
            else:
                i += 1  # a lifetime
        else:
            i += 1
    return "".join(out)


def match_close(masked, start, open_ch, close_ch):
    """Index just past the bracket that closes the one at `start`."""
    depth = 0
    for i in range(start, len(masked)):
        if masked[i] == open_ch:
            depth += 1
        elif masked[i] == close_ch:
            depth -= 1
            if depth == 0:
                return i + 1
    return len(masked)


def item_end(masked, start):
    """End of the item whose header begins at `start`: the `;` that ends it or
    the brace that closes its body, ignoring `;` inside `()`/`[]`."""
    depth = 0
    for i in range(start, len(masked)):
        c = masked[i]
        if c in "([":
            depth += 1
        elif c in ")]":
            depth -= 1
        elif c == ";" and depth <= 0:
            return i + 1
        elif c == "{" and depth <= 0:
            return match_close(masked, i, "{", "}")
    return len(masked)


def parse_pred(src):
    """A cfg predicate as a nested tuple: ('atom', name, value|None) or (op, [..])."""
    tokens = re.findall(r"\"[^\"]*\"|[A-Za-z_][\w:]*|[(),=]", src)
    pos = 0

    def expr():
        nonlocal pos
        name = tokens[pos]
        pos += 1
        if pos < len(tokens) and tokens[pos] == "(":
            pos += 1
            args = []
            while tokens[pos] != ")":
                args.append(expr())
                if tokens[pos] == ",":
                    pos += 1
            pos += 1
            return (name, args)
        if pos < len(tokens) and tokens[pos] == "=":
            value = tokens[pos + 1].strip("\"")
            pos += 2
            return ("atom", name, value)
        return ("atom", name, None)

    return expr()


def evaluate(pred, env):
    kind = pred[0]
    if kind == "all":
        return all(evaluate(p, env) for p in pred[1])
    if kind == "any":
        return any(evaluate(p, env) for p in pred[1])
    if kind == "not":
        return not evaluate(pred[1][0], env)
    _, name, value = pred
    if name in env and value is None:
        return bool(env[name])
    if name in env:
        return env[name] == value
    return True  # `test`, features, debug_assertions, arch...: on in a test build


def atoms(pred, out=None):
    out = [] if out is None else out
    if pred[0] == "atom":
        out.append((pred[1], pred[2]))
    else:
        for p in pred[1]:
            atoms(p, out)
    return out


def is_windows_skip(pred):
    return not evaluate(pred, WINDOWS) and (evaluate(pred, LINUX) or evaluate(pred, MACOS))


def canonical(pred):
    """`unix`, or a conjunction that has `unix` among its terms; or a gate on one
    specific OS (`target_os = "linux" | "macos"`) for what only that OS has."""
    if pred == ("atom", "unix", None):
        return True
    if pred[0] == "all":
        return any(canonical(p) for p in pred[1])
    if pred[0] == "atom" and pred[1] == "target_os" and pred[2] in ("linux", "macos"):
        return True
    return False


def reason_problem(reason):
    if reason is None or not reason.strip():
        return "has no reason: add `// <what Windows lacks>` on the gate line"
    if KNOWN_DEFECT.search(reason):
        return None
    if VAGUE.search(reason):
        return f"reason {reason!r} is not a cause: name the POSIX behaviour the test needs, or `see KT-<n>` for a known Windows defect, or drop the gate and let the nightly run it"
    if not CONCRETE.search(reason):
        return f"reason {reason!r} names no concrete POSIX cause (symlinks, chmod, fork/signals, unix sockets, a sh script, a program Windows lacks...); `see KT-<n>` for a known Windows defect"
    return None


class Gate:
    def __init__(self, line, pred_src, reason):
        self.line, self.reason = line, reason
        self.pred = parse_pred(pred_src)


def attribute_groups(text, masked):
    """Yield (attrs, item_start, item_end, head) for every item with attributes.
    attrs: list of (line, inner_text, raw_trailing_comment)."""
    n = len(text)
    line_starts = [0]
    for m in re.finditer("\n", text):
        line_starts.append(m.end())

    def line_of(offset):
        lo, hi = 0, len(line_starts) - 1
        while lo < hi:
            mid = (lo + hi + 1) // 2
            if line_starts[mid] <= offset:
                lo = mid
            else:
                hi = mid - 1
        return lo + 1

    attr = re.compile(r"#\s*(!?)\s*\[")
    i = 0
    while i < n:
        m = attr.search(masked, i)
        if not m:
            return
        if m.group(1):  # inner attribute: `#![cfg(..)]` for the whole file/module
            end = match_close(masked, m.end() - 1, "[", "]")
            yield ([(line_of(m.start()), text[m.end() : end - 1].strip(), trailing_comment(text, end))], None, None, "")
            i = end
            continue
        attrs = []
        j = m.start()
        while True:
            m2 = attr.match(masked, j)
            if not m2 or m2.group(1):
                break
            end = match_close(masked, m2.end() - 1, "[", "]")
            attrs.append((line_of(m2.start()), text[m2.end() : end - 1].strip(), trailing_comment(text, end)))
            j = end
            while j < n and masked[j].isspace():
                j += 1
        head = masked[j : j + 160]
        yield (attrs, j, item_end(masked, j), head)
        i = j if j > m.start() else m.end()


def trailing_comment(text, offset):
    """The `// reason` of the attribute ending at `offset`: on its own line, or, when
    rustfmt moved a trailing comment that ran past the line width, on the line right
    below it."""
    eol = text.find("\n", offset)
    rest = text[offset : len(text) if eol < 0 else eol]
    m = re.match(r"\s*//(?!/|!)\s*(.*)$", rest)
    if m:
        return m.group(1).strip()
    if rest.strip() or eol < 0:
        return None
    below_end = text.find("\n", eol + 1)
    below = text[eol + 1 : len(text) if below_end < 0 else below_end]
    m = re.match(r"\s*//(?!/|!)\s*(.*)$", below)
    return m.group(1).strip() if m else None


def cfg_pred(inner):
    m = re.match(r"cfg\((.*)\)$", inner, re.S)
    return m.group(1) if m else None


def holds_tests(path, mod_name, attrs, body):
    """Whether the module is test code: the inline `body`, or the file it names."""
    if body is not None and re.search(r"#\[(?:\w+::)*test\b", body):
        return True
    if body is None:
        target = None
        for _, inner, _ in attrs:
            p = re.match(r"path\s*=\s*\"([^\"]+)\"", inner)
            if p:
                target = path.parent / p.group(1)
        if target is None:
            for candidate in (path.parent / f"{mod_name}.rs", path.parent / mod_name / "mod.rs", path.parent / path.stem / f"{mod_name}.rs"):
                if candidate.exists():
                    target = candidate
                    break
        if target is not None and target.exists():
            return bool(re.search(r"#\[(?:\w+::)*test\b", target.read_text(encoding="utf-8", errors="replace")))
        return "test" in mod_name
    return False


def scan(path):
    findings, unix_tests = [], []
    text = path.read_text(encoding="utf-8", errors="replace")
    masked = mask(text)
    try:
        rel = path.relative_to(ROOT)
    except ValueError:
        rel = path

    def check_gates(attrs, label, line_for_item):
        """Findings for the Windows-skipping cfg attributes of one item; the reason
        found, or None when the item is not skipped on Windows."""
        found = None
        for line, inner, comment in attrs:
            ignore = re.match(r"cfg_attr\((.*)\)$", inner, re.S)
            if ignore and re.search(r"\bignore\b", ignore.group(1)):
                cond = ignore.group(1).split(",")[0]
                try:
                    if evaluate(parse_pred(cond), WINDOWS) and not evaluate(parse_pred(cond), LINUX):
                        findings.append(f"{rel}:{line}: {label}: a test skipped on Windows is `#[cfg(unix)]`, not `cfg_attr({cond}, ignore)`")
                except (IndexError, KeyError):
                    pass
                continue
            src = cfg_pred(inner)
            if src is None:
                continue
            try:
                pred = parse_pred(src)
            except IndexError:
                continue
            if not is_windows_skip(pred):
                continue
            if not canonical(pred):
                findings.append(f"{rel}:{line}: {label}: skipped on Windows through `cfg({src})`: write `#[cfg(unix)]`")
            problem = reason_problem(comment)
            if problem:
                findings.append(f"{rel}:{line}: {label}: {problem}")
            found = (line, comment or "")
        return found

    for attrs, start, end, head in attribute_groups(text, masked):
        if start is None:  # `#![cfg(..)]` on the whole file
            if holds_tests(path, path.stem, [], text):
                check_gates(attrs, f"file {path.name}", None)
            continue
        fn = FN_HEAD.match(head)
        if fn and any(TEST_ATTR.match(inner) for _, inner, _ in attrs):
            name = fn.group("name")
            gate = check_gates(attrs, name, start)
            if gate:
                unix_tests.append((rel, gate[0], name, gate[1]))
            # The raw text, because the mask blanks the `"windows"` of a target_os
            # test; a hit inside a comment or string is dropped by the mask check.
            hit = next((m for m in EARLY_RETURN.finditer(text, start, end) if masked[m.start()] != " "), None)
            if hit:
                line = text.count("\n", 0, start) + 1
                findings.append(f"{rel}:{line}: {name}: skips itself on Windows at run time; gate it with #[cfg(unix)] and a reason")
            continue
        mod = MOD_HEAD.match(head)
        if mod and any(cfg_pred(inner) is not None for _, inner, _ in attrs):
            if any((lambda src: src is not None and is_windows_skip(parse_pred(src)))(cfg_pred(inner)) for _, inner, _ in attrs if cfg_pred(inner) is not None):
                body = text[start:end] if "{" in masked[start:end] else None
                if holds_tests(path, mod.group("name"), attrs, body):
                    check_gates(attrs, f"mod {mod.group('name')}", start)
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
        print(f"{len(all_findings)} gate(s) skipped on Windows without a cfg(unix) form and a concrete reason", file=sys.stderr)
        return 1
    print(f"ok: {len(all_unix)} unix-only tests, each with a concrete reason")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

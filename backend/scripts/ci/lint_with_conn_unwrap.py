#!/usr/bin/env python3
"""CI anti-regression lint (stab-1): no `.unwrap()` inside `with_conn`
closures in production code. A panic there poisons the shared DB mutex
(recovered since 0.8.11, but still fails the request). Test code is exempt:
every item carrying `#[cfg(test)]` or `#[cfg(all(test, …))]` (the latter being
how the `full-tests` feature gates test modules) is blanked out, whether it is
a `mod tests { … }` block, a `mod tests;` line or a test-only helper. The
production code before, between and after those items is always scanned.
Run from `backend/`: `python3 scripts/ci/lint_with_conn_unwrap.py`."""
import pathlib
import re
import sys


# Matched at a position, never on a slice: slicing made the scan quadratic.
_RAW_OPEN = re.compile(r'r#*"')
_RAW_HASHES = re.compile(r'r(#*)"')
_NOT_NEWLINE = re.compile(r"[^\n]")
# `#[cfg(test)]` and `#[cfg(all(test, …))]` (feature = "full-tests", unix, …)
# mark a test-only item. `#[cfg(feature = "full-tests")]` alone does not: the
# predicate must start with `test`.
_TEST_CFG = re.compile(r"#\[cfg\((?:test|all\(test\b[^\]]*)\)\]")
_ATTRIBUTE_OPEN = re.compile(r"\s*#!?\[")
_LITERAL_START = re.compile(r"[/\"r']")
_CHAR_LITERAL = re.compile(r"'(\\(?:u\{[0-9a-fA-F]{1,6}\}|.)|[^\\'])'")


def _sanitize(text):
    """Blank out string/char literals and comments (same length, newlines
    kept so line numbers survive). Without this, a `)` inside a literal
    closed the paren scan early and hid unwraps (Codex review); it also
    prevents a literal ".unwrap()" in a string from false-positiving.
    Handles: "…" with escapes, raw strings r"…" / r#"…"#…, char literals
    (without eating lifetimes like 'a), // line and nested /* */ comments."""
    out = list(text)
    i, n = 0, len(text)

    def blank(a, b):
        out[a:b] = _NOT_NEWLINE.sub(" ", text[a:b])

    while i < n:
        c = text[i]
        two = text[i : i + 2]
        if two == "//":
            j = text.find("\n", i)
            j = n if j == -1 else j
            blank(i, j)
            i = j
        elif two == "/*":
            depth, j = 1, i + 2
            while j < n and depth:
                if text[j : j + 2] == "/*":
                    depth += 1
                    j += 2
                elif text[j : j + 2] == "*/":
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif c == '"' or (c == "r" and _RAW_OPEN.match(text, i)):
            if c == '"':
                j = i + 1
                while j < n:
                    if text[j] == "\\":
                        j += 2
                    elif text[j] == '"':
                        j += 1
                        break
                    else:
                        j += 1
            else:
                m = _RAW_HASHES.match(text, i)
                closer = '"' + m.group(1)
                j = text.find(closer, i + len(m.group(0)))
                j = n if j == -1 else j + len(closer)
            blank(i, j)
            i = j
        elif c == "'":
            # Char literal ('x', '\n', '\u{…}') vs lifetime ('a) — a char
            # literal always has a CLOSING quote within a few chars.
            m = _CHAR_LITERAL.match(text, i)
            if m:
                blank(i, m.end())
                i = m.end()
            else:
                i += 1  # lifetime: skip the quote, keep the identifier
        else:
            # Jump to the next character that can open a literal or comment.
            nxt = _LITERAL_START.search(text, i + 1)
            i = nxt.start() if nxt else n
    return "".join(out)


def _item_end(text, i):
    """Offset just past the item that starts at `i` (sanitized text, right
    after a cfg attribute). Further attributes are skipped; the item then ends
    at the first `;` outside parens/brackets, or at the `}` matching the first
    `{` outside parens/brackets. Covers `mod x;`, `use x;`, `static X = …;`,
    `mod x { … }`, `fn f() { … }`, `impl … { … }` and macro blocks."""
    n = len(text)
    while True:
        m = _ATTRIBUTE_OPEN.match(text, i)
        if not m:
            break
        j, depth = m.end(), 1
        while j < n and depth:
            if text[j] == "[":
                depth += 1
            elif text[j] == "]":
                depth -= 1
            j += 1
        i = j
    nested = 0
    j = i
    while j < n:
        c = text[j]
        if c in "([":
            nested += 1
        elif c in ")]":
            nested -= 1
        elif c == ";" and nested <= 0:
            return j + 1
        elif c == "{" and nested <= 0:
            depth, j = 1, j + 1
            while j < n and depth:
                if text[j] == "{":
                    depth += 1
                elif text[j] == "}":
                    depth -= 1
                j += 1
            return j
        j += 1
    return n


def _exempt_test_items(text):
    """Blank every test-only item (same length, newlines kept): the item that
    follows each `#[cfg(test)]` / `#[cfg(all(test, …))]` attribute, and only
    that item. Earlier versions cut the file at the first attribute, which
    hid every production `with_conn` placed after a `mod tests;` line."""
    out = list(text)
    pos = 0
    while True:
        m = _TEST_CFG.search(text, pos)
        if not m:
            break
        end = _item_end(text, m.end())
        out[m.start() : end] = _NOT_NEWLINE.sub(" ", text[m.start() : end])
        pos = end
    return "".join(out)


def find_violations(text, path="<mem>"):
    """`path:line` for every `.unwrap()` inside a `with_conn(...)` call.

    Runs on SANITIZED text (strings/comments blanked, then test-only items
    blanked) and scans to the BALANCED closing paren with no length cap
    (Codex review, findings 2+3: a 4k cap skipped long closures; literal
    parens broke the balance). An unbalanced call is a hard error — loud
    beats a silent blind spot.
    """
    viol = []
    prod = _exempt_test_items(_sanitize(text))
    for m in re.finditer(r"with_conn(?:_blocking)?\s*\(", prod):
        i, depth, end = m.end() - 1, 0, None
        for j in range(i, len(prod)):
            c = prod[j]
            if c == "(":
                depth += 1
            elif c == ")":
                depth -= 1
                if depth == 0:
                    end = j
                    break
        if end is None:
            raise RuntimeError(
                f"{path}: unbalanced parens after with_conn at offset {i} — lint cannot scan"
            )
        for um in re.finditer(r"\.unwrap\(\)", prod[i:end]):
            line = prod[: i + um.start()].count("\n") + 1
            viol.append(f"{path}:{line}")
    return viol


def main():
    viol = []
    for f in pathlib.Path("src").rglob("*.rs"):
        viol.extend(find_violations(f.read_text(), str(f)))
    if viol:
        print("::error::unwrap() inside a with_conn closure panics the DB request — return an error instead:")
        print("\n".join(viol))
        sys.exit(1)
    print("OK: no unwrap() inside with_conn closures")


if __name__ == "__main__":
    main()

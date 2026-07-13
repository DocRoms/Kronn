#!/usr/bin/env python3
"""CI anti-regression lint (stab-1): no `.unwrap()` inside `with_conn`
closures in production code. A panic there poisons the shared DB mutex
(recovered since 0.8.11, but still fails the request). Test modules
(everything after the first `#[cfg(test)]` in a file) are exempt.
Run from `backend/`: `python3 scripts/ci/lint_with_conn_unwrap.py`."""
import pathlib
import re
import sys


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
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = " "

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
        elif c == '"' or (c == "r" and re.match(r'r#*"', text[i:])):
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
                m = re.match(r'r(#*)"', text[i:])
                closer = '"' + m.group(1)
                j = text.find(closer, i + len(m.group(0)))
                j = n if j == -1 else j + len(closer)
            blank(i, j)
            i = j
        elif c == "'":
            # Char literal ('x', '\n', '\u{…}') vs lifetime ('a) — a char
            # literal always has a CLOSING quote within a few chars.
            m = re.match(r"'(\\(?:u\{[0-9a-fA-F]{1,6}\}|.)|[^\\'])'", text[i:])
            if m:
                blank(i, i + m.end())
                i += m.end()
            else:
                i += 1  # lifetime: skip the quote, keep the identifier
        else:
            i += 1
    return "".join(out)


def find_violations(text, path="<mem>"):
    """`path:line` for every `.unwrap()` inside a `with_conn(...)` call.

    Runs on SANITIZED text (strings/comments blanked) and scans to the
    BALANCED closing paren with no length cap (Codex review, findings 2+3:
    a 4k cap skipped long closures; literal parens broke the balance).
    An unbalanced call is a hard error — loud beats a silent blind spot.
    """
    viol = []
    prod = _sanitize(re.split(r"#\[cfg\(test\)\]", text)[0])
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

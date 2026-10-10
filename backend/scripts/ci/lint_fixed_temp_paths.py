#!/usr/bin/env python3
"""CI lint (KT-800): no fixed path under the system temp directory.
Two `cargo test` runs at once (two worktrees, or nextest processes) share
`temp_dir()`, so a fixed name there makes them clobber each other. Use
`tempfile::tempdir()`, or a name carrying the process id or a UUID.
Run from `backend/`: `python3 scripts/ci/lint_fixed_temp_paths.py`."""
import importlib.util
import pathlib
import re
import sys

# Same tokenizer as the with_conn lint: blanks string/char literals and
# comments (same length, newlines kept), so only code tokens remain.
_spec = importlib.util.spec_from_file_location(
    "lint_wcu", pathlib.Path(__file__).parent / "lint_with_conn_unwrap.py")
_wcu = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_wcu)
sanitize = _wcu._sanitize

_JOIN = re.compile(r"temp_dir\(\)\s*\.join\s*\(")
_FORMAT = re.compile(r"format!\s*\(")
# Matched on code tokens only: text inside strings or comments never counts.
_UNIQUE = re.compile(
    r"process::id\s*\(|Uuid::new_v4\s*\(|\bas_nanos\s*\(|\bnanos\b|\bfetch_add\s*\(|\btempfile\b"
)
_FS_CALL = re.compile(
    r"fs::(?:create_dir(?:_all)?|write|remove_(?:dir_all|dir|file)|rename|copy)\s*\("
)
_TMP_LITERAL = re.compile(r'\s*(?:b|r#*|br#*)?"/tmp/')


def _close(code, open_paren):
    """Index of the paren closing `code[open_paren]`, on sanitized text."""
    depth = 0
    for i in range(open_paren, len(code)):
        if code[i] == "(":
            depth += 1
        elif code[i] == ")":
            depth -= 1
            if depth == 0:
                return i
    return len(code)


def _line(text, pos):
    return text.count("\n", 0, pos) + 1


def violations(text):
    """(line, snippet) for each fixed temp path in `text`."""
    code = sanitize(text)
    found = []
    for m in _JOIN.finditer(code):
        end = _close(code, m.end() - 1)
        arg_code = code[m.end():end]
        snippet = text[m.start():end + 1].replace("\n", " ")
        if not arg_code.strip():
            # Only a literal: a plain string is never interpolated, braces or not.
            found.append((_line(text, m.start()), snippet))
            continue
        fmt = _FORMAT.match(arg_code.lstrip())
        if fmt:
            if not _UNIQUE.search(arg_code):
                found.append((_line(text, m.start()), snippet))
    for m in _FS_CALL.finditer(code):
        if _TMP_LITERAL.match(text, m.end()):
            end = _close(code, m.end() - 1)
            found.append((_line(text, m.start()), text[m.start():end + 1]))
    return sorted(found)


def main(root="."):
    bad = []
    for sub in ("src", "tests"):
        for path in sorted(pathlib.Path(root, sub).rglob("*.rs")):
            for line, snippet in violations(path.read_text(encoding="utf-8")):
                bad.append(f"{path}:{line}: {snippet}")
    if bad:
        print("Fixed path under the system temp dir (use tempfile::tempdir()):")
        print("\n".join(bad))
        return 1
    print("lint_fixed_temp_paths: OK")
    return 0


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:]))

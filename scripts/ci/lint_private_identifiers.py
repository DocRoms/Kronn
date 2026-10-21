#!/usr/bin/env python3
"""CI lint (KT-1413): no private third-party repository identifier in the public tree.

Kronn is public. Its audit benches ran against private repositories, and test
fixtures, comments and prompts must not carry those repositories' names or the
paths of the secrets committed in them. Use neutral stand-ins (`.ssh/deploy-key`,
`keys/service-key`, "a Node service", "a PHP site").

The deny-list is a set of SHA-256 digests, so this file does not re-publish what it
guards. Every tracked text file is split into tokens (lowercased; whitespace, quotes,
backticks, brackets, `:` and `,` separate them). Each token is checked as is, then
per run of path components, then per word (runs of letters and digits), so a name is caught
inside a path, a snake_case identifier or a possessive. A token whose digest is listed
fails the run; the report gives file, line and a digest prefix, never the token.

To deny a new identifier, add the hex digest of its lowercase form:
`python3 -c "import hashlib;print(hashlib.sha256(b'<token>').hexdigest())"`.
Run from anywhere: `python3 scripts/ci/lint_private_identifiers.py [root]`."""
import hashlib
import pathlib
import re
import subprocess
import sys

DENY_DIGESTS = frozenset({
    # Names of two private repositories used as audit benches.
    "8d3aa1a6f227d714692a9d5a7fbbda496fb09f17f7207a11ffd0a4cca6cf35b7",
    "500af7989bd1856cb3c49be3c60094dd01b39d69d79171a3c3a726b0e2c86d47",
    # Host name of a private-key path committed in one of them.
    "0856fd1953de31f9ea06e896883bc1f08f4dd31c05bf4a79b1de535c0e632779",
    # Directory holding the private keys committed in the other.
    "2686205ae18e28351fd89dac780e9354394e2a56530df39042c82a977a4dcbdf",
})

_COARSE = re.compile(r"""[\s"'`()\[\]{}<>:,;]+""")
_WORDS = re.compile(r"[^a-z0-9]+")

SKIP_NAMES = {"Cargo.lock", "pnpm-lock.yaml", "package-lock.json", "yarn.lock"}
MAX_BYTES = 2_000_000


def digest(token):
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


def _variants(token):
    """The token, each run of consecutive path components, and the words."""
    yield token
    parts = token.split("/")
    for i in range(len(parts)):
        for j in range(i + 1, len(parts) + 1):
            yield "/".join(parts[i:j])
        yield from _WORDS.split(parts[i])


def violations(text, digests=DENY_DIGESTS):
    """(line, digest) for each token of `text` whose digest is in `digests`."""
    found = set()
    for number, line in enumerate(text.splitlines(), 1):
        for coarse in _COARSE.split(line.lower()):
            for token in _variants(coarse.strip(".")):
                if token and (d := digest(token)) in digests:
                    found.add((number, d))
    return sorted(found)


def tracked_files(root):
    """Tracked files under `root` (relative posix paths); a plain walk outside git."""
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "ls-files", "-z"],
            check=True, capture_output=True,
        ).stdout.decode("utf-8", "replace")
        return [p for p in out.split("\0") if p]
    except (OSError, subprocess.CalledProcessError):
        return [
            p.relative_to(root).as_posix()
            for p in sorted(pathlib.Path(root).rglob("*"))
            if p.is_file() and ".git" not in p.relative_to(root).parts
        ]


def scan(root, digests=DENY_DIGESTS):
    bad = []
    for rel in tracked_files(root):
        path = pathlib.Path(root, rel)
        if path.name in SKIP_NAMES or not path.is_file():
            continue
        try:
            if path.stat().st_size > MAX_BYTES:
                continue
            text = path.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError):
            continue
        for line, d in violations(text, digests):
            bad.append(f"{rel}:{line}: private identifier (sha256 {d[:12]})")
    return bad


def main(root=None, digests=DENY_DIGESTS):
    root = pathlib.Path(root) if root else pathlib.Path(__file__).resolve().parents[2]
    bad = scan(root, digests)
    if bad:
        print("Private third-party identifier in the public tree (use a neutral stand-in):")
        print("\n".join(bad))
        return 1
    print("No private third-party repository identifier found.")
    return 0


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:2]))

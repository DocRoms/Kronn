#!/usr/bin/env python3
"""Build a desktop release body from the installers the release really carries.

One direct link per attached installer, the version's CHANGELOG section above
the install table. A platform without an installer or a version without a
CHANGELOG section fails, so a draft never names a file it does not have.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_URL = "https://github.com/DocRoms/Kronn"

# Fallback handles kept as real mentions when no contributor list is given
# (the human maintainer). Every other bare @name is wrapped in a code span:
# GitHub would otherwise notify that account and list it under Contributors.
MENTION_ALLOWLIST = frozenset({"docroms"})

INSTALLER_SUFFIXES = (".exe", ".dmg", ".deb")

# GitHub refuses a release body over 125,000 characters.
MAX_BODY_CHARS = 125_000

# (label, min version, file-name endings in display order); mirrors
# RELEASE_PLATFORMS in backend/sidecars/docs/verify_artifacts.py (Tauri targets
# deb, nsis, dmg).
PLATFORMS = (
    ("Windows", "Windows 10+", (".exe",)),
    ("macOS Apple Silicon", "macOS 11+", ("_aarch64.dmg",)),
    ("macOS Intel", "macOS 11+", ("_x64.dmg",)),
    ("Linux", "Ubuntu 22.04+ / Debian 12+ / Fedora 38+", (".deb",)),
)

WEBKIT_NOTE = """### Linux — webkit2gtk-4.1 requirement

Kronn uses Tauri 2 which requires **`libwebkit2gtk-4.1-0`** at runtime.
On Ubuntu 22.04 it lives in the `universe` repository (enabled by
default on desktop installs). On minimal/server installs you may need:
```bash
sudo add-apt-repository universe
sudo apt update
sudo apt install -y libwebkit2gtk-4.1-0
sudo dpkg -i Kronn_*_amd64.deb
sudo apt-get -f install   # resolves any remaining deps
```
On older distros (Ubuntu 20.04, RHEL 8) the `.deb` will not install"""

GATEKEEPER_NOTE = """### macOS — "app is damaged" warning

Kronn is open-source and not signed with an Apple Developer certificate.
macOS Gatekeeper may show _"Kronn is damaged and can't be opened"_.

**Fix** — run this once after installing:
```bash
xattr -cr /Applications/Kronn.app
```
Then open the app normally."""


def changelog_section(text: str, version: str) -> str:
    lines = text.splitlines()
    head = re.compile(r"^## \[" + re.escape(version) + r"\]")
    start = next((i for i, line in enumerate(lines) if head.match(line)), None)
    if start is None:
        raise SystemExit(f"CHANGELOG.md has no section for {version}")
    end = next(
        (i for i in range(start + 1, len(lines)) if lines[i].startswith("## [")),
        len(lines),
    )
    body = "\n".join(lines[start + 1 : end]).strip()
    if not body:
        raise SystemExit(f"CHANGELOG.md section for {version} is empty")
    return body


_FENCE = re.compile(r"^\s*(`{3,}|~{3,})")
_CODE_SPAN = re.compile(r"(?<!`)(`+)(?!`).*?(?<!`)\1(?!`)", re.DOTALL)
# URLs are skipped whole; an @ glued to a word (e-mail) never matches.
_URL_OR_MENTION = re.compile(
    r"(?P<url>https?://[^\s<>)\]]+)"
    r"|(?:(?P<tick>`+)|(?<![\w@/.\-]))(?P<mention>@[A-Za-z0-9](?:[A-Za-z0-9-]*[A-Za-z0-9])?(?:/[\w.-]+)?)"
)


def _wrap_mentions(text: str, allow: frozenset[str]) -> str:
    def repl(m: re.Match[str]) -> str:
        mention = m.group("mention")
        if mention is None or mention[1:].lower() in allow:
            return m.group(0)
        # Text outside valid spans: a backtick here is a lone one. Escape it so
        # it cannot pair with the wrapper's backtick and leave the mention bare.
        tick = "\\`" * len(m.group("tick") or "")
        return f"{tick}`{mention}`"

    out: list[str] = []
    pos = 0
    for span in _CODE_SPAN.finditer(text):
        out.append(_URL_OR_MENTION.sub(repl, text[pos : span.start()]))
        out.append(span.group(0))
        pos = span.end()
    out.append(_URL_OR_MENTION.sub(repl, text[pos:]))
    return "".join(out)


def neutralize_mentions(text: str, allow: frozenset[str] = MENTION_ALLOWLIST) -> str:
    """Wrap every bare @name outside fences and code spans; other text is untouched."""
    out: list[str] = []
    prose: list[str] = []
    fence: str | None = None

    def flush() -> None:
        if prose:
            out.append(_wrap_mentions("".join(prose), allow))
            prose.clear()

    for line in text.splitlines(keepends=True):
        m = _FENCE.match(line)
        if fence is None:
            if m:
                flush()
                fence = m.group(1)
                out.append(line)
            else:
                prose.append(line)
        else:
            out.append(line)
            if m and m.group(1)[0] == fence[0] and len(m.group(1)) >= len(fence):
                fence = None
    flush()
    return "".join(out)


def parse_allowlist(handles: list[str]) -> frozenset[str]:
    """Normalise handles (case-insensitive, optional @); empty falls back to the maintainer."""
    names = {h.strip().lstrip("@").lower() for h in handles if h.strip()}
    if not names:
        print("warning: no contributor handle given, allowing the maintainer only", file=sys.stderr)
        return MENTION_ALLOWLIST
    return frozenset(names)


def build_body(tag: str, changelog: str, assets: list[str], allow: frozenset[str] = MENTION_ALLOWLIST) -> str:
    version = tag[1:] if tag.startswith("v") else tag
    names = sorted({name for name in assets if name.lower().endswith(INSTALLER_SUFFIXES)})
    rows: list[str] = []
    missing: list[str] = []
    for label, minimum, endings in PLATFORMS:
        files = [n for e in endings for n in names if n.lower().endswith(e)]
        if not files:
            missing.append(label)
            continue
        links = " or ".join(f"[`{n}`]({REPO_URL}/releases/download/{tag}/{n})" for n in files)
        rows.append(f"| {label} | {links} | {minimum} |")
    if missing:
        raise SystemExit("No installer attached for: " + ", ".join(missing))

    linux = WEBKIT_NOTE + ", so upgrade."
    install = "\n".join(
        [
            "",
            "## Installation",
            "",
            "| OS | Download | Min version |",
            "|----|----------|-------------|",
            *rows,
            "",
            linux,
            "",
            GATEKEEPER_NOTE,
            "",
        ]
    )
    notes = fit_notes(neutralize_mentions(changelog_section(changelog, version), allow), tag, MAX_BODY_CHARS - len(install) - 1)
    return notes + "\n" + install


def fit_notes(notes: str, tag: str, budget: int) -> str:
    """The notes whole, or cut before the first entry that does not fit, outside any code fence."""
    if len(notes) <= budget:
        return notes
    more = f"\n\n…and more: the full notes are in [CHANGELOG.md]({REPO_URL}/blob/{tag}/CHANGELOG.md)."
    limit = budget - len(more)
    cut = 0
    fenced = False
    offset = 0
    for line in notes.splitlines(keepends=True):
        if line.lstrip().startswith(("```", "~~~")):
            fenced = not fenced
        if not fenced and offset <= limit and (line.startswith("- ") or line.startswith("#")):
            cut = offset
        offset += len(line)
    if cut == 0:
        raise SystemExit(f"The CHANGELOG section of {tag} cannot be cut to fit a release body")
    return notes[:cut].rstrip() + more


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--changelog", type=Path, required=True)
    parser.add_argument("--assets-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--allow-mention", action="append", default=[], metavar="LOGIN")
    parser.add_argument("--allow-mentions-file", type=Path, help="one login per line")
    args = parser.parse_args()
    handles = list(args.allow_mention)
    if args.allow_mentions_file:
        try:
            handles += args.allow_mentions_file.read_text(encoding="utf-8").split()
        except OSError as err:
            print(f"warning: cannot read {args.allow_mentions_file}: {err}", file=sys.stderr)
    allow = parse_allowlist(handles)
    assets = [p.name for p in args.assets_dir.rglob("*") if p.is_file() and p.stat().st_size > 0]
    body = build_body(args.tag, args.changelog.read_text(encoding="utf-8"), assets, allow)
    if args.output:
        args.output.write_text(body, encoding="utf-8")
    else:
        sys.stdout.write(body)


if __name__ == "__main__":
    main()

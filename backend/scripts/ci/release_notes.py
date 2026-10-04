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

INSTALLER_SUFFIXES = (".exe", ".dmg", ".deb")

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


def build_body(tag: str, changelog: str, assets: list[str]) -> str:
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
    return "\n".join(
        [
            changelog_section(changelog, version),
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


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", required=True)
    parser.add_argument("--changelog", type=Path, required=True)
    parser.add_argument("--assets-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    assets = [p.name for p in args.assets_dir.rglob("*") if p.is_file() and p.stat().st_size > 0]
    body = build_body(args.tag, args.changelog.read_text(encoding="utf-8"), assets)
    if args.output:
        args.output.write_text(body, encoding="utf-8")
    else:
        sys.stdout.write(body)


if __name__ == "__main__":
    main()

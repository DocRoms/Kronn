#!/usr/bin/env python3
"""Fail a desktop release unless every platform produced a real installer."""

from __future__ import annotations

import argparse
from pathlib import Path


EXPECTED_ARTIFACTS = {
    "kronn-windows": {".exe", ".msi"},
    "kronn-macOS-arm64": {".dmg"},
    "kronn-macOS-x64": {".dmg"},
    "kronn-linux": {".deb", ".appimage"},
}


def verify(root: Path) -> list[Path]:
    installers: list[Path] = []
    failures: list[str] = []

    for artifact, extensions in EXPECTED_ARTIFACTS.items():
        directory = root / artifact
        candidates = [
            path
            for path in directory.rglob("*")
            if path.is_file() and path.suffix.lower() in extensions
        ] if directory.is_dir() else []
        nonempty = [path for path in candidates if path.stat().st_size > 0]
        if not nonempty:
            expected = ", ".join(sorted(extensions))
            failures.append(f"{artifact}: no non-empty installer ({expected})")
        installers.extend(nonempty)

    if failures:
        raise SystemExit("Incomplete desktop installer matrix:\n- " + "\n- ".join(failures))

    return installers


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    args = parser.parse_args()
    installers = verify(args.root.resolve())
    for installer in installers:
        print(f"verified {installer} ({installer.stat().st_size} bytes)")


if __name__ == "__main__":
    main()

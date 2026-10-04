#!/usr/bin/env python3
"""Fail a desktop release unless every platform produced a real installer.

Two checks: the CI artifacts before the release is created, and the assets the
published release really carries (`--release-assets`, names on stdin). Releases
0.12.0 to 0.14.1 shipped with no installer at all, which only the second can
catch (KT-970).
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


# Exactly the bundle targets of desktop/src-tauri/tauri.conf.json (deb, nsis,
# dmg): an installer Tauri is not configured to build must not satisfy a check.
EXPECTED_ARTIFACTS = {
    "kronn-windows": {".exe"},
    "kronn-macOS-arm64": {".dmg"},
    "kronn-macOS-x64": {".dmg"},
    "kronn-linux": {".deb"},
}


# What each platform needs among a release's assets, by Tauri's file names
# (`Kronn_0.14.2_aarch64.dmg`, `Kronn_0.14.2_x64-setup.exe`, ...).
RELEASE_PLATFORMS = {
    "Windows": (".exe",),
    "macOS Apple Silicon": ("_aarch64.dmg",),
    "macOS Intel": ("_x64.dmg",),
    "Linux": (".deb",),
}


def missing_release_platforms(asset_names: list[str]) -> list[str]:
    names = [name.strip().lower() for name in asset_names if name.strip()]
    return [
        platform
        for platform, endings in RELEASE_PLATFORMS.items()
        if not any(name.endswith(ending) for name in names for ending in endings)
    ]


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
    parser.add_argument("root", type=Path, nargs="?")
    parser.add_argument(
        "--release-assets",
        action="store_true",
        help="read a release's asset names on stdin and fail if a platform has no installer",
    )
    args = parser.parse_args()
    if args.release_assets:
        missing = missing_release_platforms(sys.stdin.read().splitlines())
        if missing:
            raise SystemExit("Release without an installer for: " + ", ".join(missing))
        print("release carries an installer for every platform")
        return
    if args.root is None:
        parser.error("the artifact root is required")
    installers = verify(args.root.resolve())
    for installer in installers:
        print(f"verified {installer} ({installer.stat().st_size} bytes)")


if __name__ == "__main__":
    main()

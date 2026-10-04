#!/usr/bin/env python3
"""Tests for the desktop release body generator (KT-1004)."""

from __future__ import annotations

import fnmatch
import importlib.util
import pathlib
import re
import subprocess
import sys
import tempfile
import unittest

_SPEC = importlib.util.spec_from_file_location(
    "release_notes", pathlib.Path(__file__).with_name("release_notes.py")
)
release_notes = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(release_notes)

# Tauri targets deb, nsis, dmg (desktop/src-tauri/tauri.conf.json).
FULL = [
    "Kronn_1.2.3_x64-setup.exe",
    "Kronn_1.2.3_aarch64.dmg",
    "Kronn_1.2.3_x64.dmg",
    "Kronn_1.2.3_amd64.deb",
]
CHANGELOG = """# Changelog

## [Unreleased]

## [1.2.3] - 2026-01-01

### Added

- A thing (KT-1).

## [1.2.2] - 2025-12-01

- Older entry.
"""


def links(body: str) -> list[str]:
    return re.findall(r"\(https://github\.com/DocRoms/Kronn/releases/download/[^)]+\)", body)


class ReleaseNotesTest(unittest.TestCase):
    def test_every_link_matches_an_attached_file(self):
        body = release_notes.build_body("1.2.3", CHANGELOG, FULL)
        found = links(body)
        self.assertEqual(len(found), len(FULL))
        for name in FULL:
            self.assertIn(f"({release_notes.REPO_URL}/releases/download/1.2.3/{name})", found)

    def test_nothing_unattached_is_linked(self):
        extra = FULL + ["Kronn_1.2.3_x64_en-US.msi", "Kronn_1.2.3_amd64.AppImage"]
        body = release_notes.build_body("1.2.3", CHANGELOG, extra)
        self.assertEqual(len(links(body)), len(FULL))
        self.assertNotIn(".msi", body)
        self.assertNotIn("AppImage", body)

    def test_dpkg_command_matches_the_real_file_name(self):
        body = release_notes.build_body("1.2.3", CHANGELOG, FULL)
        pattern = re.search(r"dpkg -i (\S+)", body).group(1)
        self.assertTrue(fnmatch.fnmatchcase("Kronn_1.2.3_amd64.deb", pattern), pattern)

    def test_msi_or_appimage_alone_do_not_stand_for_a_platform(self):
        for label, name in (("windows", "Kronn_1.2.3_x64-setup.exe"), ("linux", "Kronn_1.2.3_amd64.deb")):
            attached = [n for n in FULL if n != name] + [
                "Kronn_1.2.3_x64_en-US.msi", "Kronn_1.2.3_amd64.AppImage"
            ]
            with self.assertRaises(SystemExit, msg=label):
                release_notes.build_body("1.2.3", CHANGELOG, attached)

    def test_missing_platform_fails(self):
        platforms = {
            "windows": (".exe",),
            "arm": ("_aarch64.dmg",),
            "intel": ("_x64.dmg",),
            "linux": (".deb",),
        }
        for label, suffixes in platforms.items():
            attached = [n for n in FULL if not n.endswith(suffixes)]
            with self.assertRaises(SystemExit, msg=label):
                release_notes.build_body("1.2.3", CHANGELOG, attached)

    def test_missing_changelog_section_fails(self):
        with self.assertRaises(SystemExit):
            release_notes.build_body("9.9.9", CHANGELOG, FULL)

    def test_changelog_section_is_bounded_and_above_the_table(self):
        body = release_notes.build_body("1.2.3", CHANGELOG, FULL)
        self.assertIn("A thing (KT-1).", body)
        self.assertNotIn("Older entry", body)
        self.assertLess(body.index("A thing"), body.index("## Installation"))
        self.assertIn("xattr -cr", body)
        self.assertIn("libwebkit2gtk-4.1-0", body)

    def test_cli_exit_codes(self):
        script = pathlib.Path(release_notes.__file__)
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "CHANGELOG.md").write_text(CHANGELOG, encoding="utf-8")
            assets = root / "artifacts" / "kronn-linux"
            assets.mkdir(parents=True)
            for name in FULL:
                (assets / name).write_text("x")
            cmd = [sys.executable, str(script), "--tag", "1.2.3",
                   "--changelog", str(root / "CHANGELOG.md"),
                   "--assets-dir", str(root / "artifacts")]
            self.assertEqual(subprocess.run(cmd, capture_output=True).returncode, 0)
            (assets / FULL[2]).unlink()
            self.assertNotEqual(subprocess.run(cmd, capture_output=True).returncode, 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)

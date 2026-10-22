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

    def test_an_oversized_section_is_cut_between_entries_and_links_the_changelog(self):
        entry = "- " + "x" * 998 + "\n"
        fenced = "- fenced:\n```\n" + "y" * 500 + "\n- not an entry\n```\n"
        big = CHANGELOG.replace("- A thing (KT-1).\n", "- A thing (KT-1).\n" + fenced + entry * 200)
        body = release_notes.build_body("1.2.3", big, FULL)
        self.assertLessEqual(len(body), release_notes.MAX_BODY_CHARS)
        self.assertIn("A thing (KT-1).", body)
        self.assertIn("/blob/1.2.3/CHANGELOG.md", body)
        self.assertIn("## Installation", body)
        notes = body[: body.index("…and more")]
        self.assertEqual(notes.count("```") % 2, 0)
        self.assertTrue(notes.rstrip().endswith("x"))

    def test_the_real_0151_section_fits_a_release_body(self):
        changelog = (pathlib.Path(__file__).parents[3] / "CHANGELOG.md").read_text(encoding="utf-8")
        body = release_notes.build_body("0.15.1", changelog, FULL)
        self.assertLessEqual(len(body), release_notes.MAX_BODY_CHARS)
        self.assertIn("ComfyUI", body)

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

    def test_plain_aliases_are_wrapped(self):
        text = 'Say "@opencode lance @codex et @claude" or (@a, @b).'
        self.assertEqual(
            release_notes.neutralize_mentions(text),
            'Say "`@opencode` lance `@codex` et `@claude`" or (`@a`, `@b`).',
        )

    def test_email_is_untouched(self):
        text = "Write to a@b.com or first.last@corp.example.org."
        self.assertEqual(release_notes.neutralize_mentions(text), text)

    def test_url_with_at_is_untouched(self):
        text = "See https://user@host.example/x and https://medium.com/@someone/post now."
        self.assertEqual(release_notes.neutralize_mentions(text), text)

    def test_existing_code_is_untouched(self):
        text = (
            "Inline `@codex` and ``@a `x` @b`` stay.\n"
            "```bash\n@claude run\n```\n"
            "~~~\n@litellm\n~~~\n"
            "- tail `span across\n  lines @z` end @out\n"
        )
        want = text.replace("end @out", "end `@out`")
        self.assertEqual(release_notes.neutralize_mentions(text), want)

    def test_allowlisted_handle_is_untouched(self):
        text = "Thanks @DocRoms and @docroms, not @codex."
        self.assertEqual(
            release_notes.neutralize_mentions(text),
            "Thanks @DocRoms and @docroms, not `@codex`.",
        )

    def test_build_body_wraps_only_the_changelog_section(self):
        log = CHANGELOG.replace("A thing (KT-1).", 'A thing "@a et @b" (KT-1).')
        body = release_notes.build_body("1.2.3", log, FULL)
        self.assertIn('"`@a` et `@b`"', body)
        self.assertNotIn(" @a", body)

    def test_real_0150_section_matches_the_hand_fixed_body(self):
        # The 0.15.0 release was fixed by hand: exactly these 12 aliases wrapped
        # in code, every other byte unchanged.
        hand_fixed = ["@opencode", "@codex", "@claude", "@codex", "@claude", "@opencode",
                      "@codex", "@litellm", "@a", "@b", "@a", "@b"]
        root = pathlib.Path(__file__).resolve().parents[3]
        section = release_notes.changelog_section(
            (root / "CHANGELOG.md").read_text(encoding="utf-8"), "0.15.0"
        )
        out = release_notes.neutralize_mentions(section)
        self.assertEqual(re.findall(r"`(@\w+)`", out), hand_fixed)
        self.assertEqual(re.sub(r"`(@\w+)`", r"\1", out), section)
        self.assertEqual(release_notes.neutralize_mentions(out), out)

    def test_contributor_handle_stays_a_mention_and_alias_is_wrapped(self):
        allow = release_notes.parse_allowlist(["@Alice", "bob"])
        out = release_notes.neutralize_mentions("@alice and @BOB ask @codex", allow)
        self.assertEqual(out, "@alice and @BOB ask `@codex`")

    def test_empty_list_falls_back_to_the_maintainer(self):
        self.assertEqual(release_notes.parse_allowlist([]), release_notes.MENTION_ALLOWLIST)
        self.assertEqual(release_notes.parse_allowlist(["", "  "]), release_notes.MENTION_ALLOWLIST)

    def test_lone_backtick_before_a_mention_does_not_leave_it_bare(self):
        out = release_notes.neutralize_mentions("see `@user here")
        self.assertEqual(out, "see \\``@user` here")
        out = release_notes.neutralize_mentions("a`@other end")
        self.assertEqual(out, "a\\``@other` end")
        self.assertEqual(release_notes.neutralize_mentions("`@ok` fine"), "`@ok` fine")

    def test_cli_allowlist_options_and_missing_file(self):
        script = pathlib.Path(release_notes.__file__)
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            log = CHANGELOG.replace("A thing (KT-1).", "Ask @alice, @carol and @codex.")
            (root / "CHANGELOG.md").write_text(log, encoding="utf-8")
            assets = root / "artifacts"
            assets.mkdir()
            for name in FULL:
                (assets / name).write_text("x")
            (root / "c.txt").write_text("alice\n", encoding="utf-8")
            base = [sys.executable, str(script), "--tag", "1.2.3",
                    "--changelog", str(root / "CHANGELOG.md"), "--assets-dir", str(assets)]
            run = lambda *extra: subprocess.run(base + list(extra), capture_output=True, text=True)
            r = run("--allow-mentions-file", str(root / "c.txt"), "--allow-mention", "@Carol")
            self.assertEqual(r.returncode, 0)
            self.assertIn("Ask @alice, @carol and `@codex`.", r.stdout)
            r = run("--allow-mentions-file", str(root / "missing.txt"))
            self.assertEqual(r.returncode, 0)
            self.assertIn("`@alice`", r.stdout)
            self.assertIn("warning", r.stderr)

    def test_workflow_lists_contributors_before_the_body_and_passes_them(self):
        wf = (pathlib.Path(__file__).resolve().parents[3] / ".github/workflows/desktop-build.yml").read_text()
        lookup, body = wf.index("repos/$GH_REPO/contributors"), wf.index("release_notes.py")
        self.assertLess(lookup, body)
        self.assertIn("--allow-mentions-file contributors.txt", wf)
        self.assertIn("endswith(\"[bot]\")", wf)
        self.assertIn("|| echo", wf[lookup:body])


if __name__ == "__main__":
    unittest.main(verbosity=2)

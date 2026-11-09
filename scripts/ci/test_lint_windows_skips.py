#!/usr/bin/env python3
"""Tests for lint_windows_skips.py (KT-1629). Run: python3 scripts/ci/test_lint_windows_skips.py"""
import tempfile
import unittest
from pathlib import Path

import lint_windows_skips as lint


def scan(source):
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "sample.rs"
        path.write_text(source, encoding="utf-8")
        return lint.scan(path)


class WindowsSkipLint(unittest.TestCase):
    def test_a_gate_with_a_trailing_reason_is_clean(self):
        findings, unix = scan("#[cfg(unix)] // creates symlinks\n#[test]\nfn a() {\n}\n")
        self.assertEqual(findings, [])
        self.assertEqual([(name, reason) for _, _, name, reason in unix], [("a", "creates symlinks")])

    def test_a_plain_comment_above_the_gate_is_a_reason(self):
        findings, _ = scan("// POSIX mode bits\n#[cfg(unix)]\n#[test]\nfn a() {\n}\n")
        self.assertEqual(findings, [])

    def test_a_doc_comment_is_not_a_reason(self):
        findings, _ = scan("/// What the test proves.\n#[cfg(unix)]\n#[test]\nfn a() {\n}\n")
        self.assertEqual(len(findings), 1)
        self.assertIn("without a reason", findings[0])

    def test_a_bare_gate_is_refused_on_a_tokio_test_too(self):
        findings, _ = scan("#[tokio::test]\n#[cfg(unix)]\nasync fn a() {\n}\n")
        self.assertEqual(len(findings), 1)

    def test_a_gate_on_something_that_is_not_a_test_is_not_checked(self):
        findings, _ = scan("#[cfg(unix)]\nfn helper() {\n}\n")
        self.assertEqual(findings, [])

    def test_other_ways_to_skip_on_windows_are_refused(self):
        for gate in ("#[cfg(not(windows))]", "#[cfg_attr(windows, ignore)]", "#[cfg_attr(not(unix), ignore)]"):
            findings, _ = scan(f"{gate}\n#[test]\nfn a() {{\n}}\n")
            self.assertEqual(len(findings), 1, gate)

    def test_a_test_that_returns_early_on_windows_is_refused(self):
        findings, _ = scan("#[test]\nfn a() {\n    if cfg!(windows) { return; }\n}\n")
        self.assertEqual(len(findings), 1)
        self.assertIn("at run time", findings[0])

    def test_an_early_return_in_a_later_test_is_not_blamed_on_this_one(self):
        source = "mod t {\n    #[test]\n    fn a() {\n    }\n\n    #[test]\n    fn b() {\n        if cfg!(windows) { return; }\n    }\n}\n"
        findings, _ = scan(source)
        self.assertEqual([f.split(": ")[1] for f in findings], ["b"])

    def test_the_repository_is_clean(self):
        findings = []
        for path in lint.rust_files():
            findings += lint.scan(path)[0]
        self.assertEqual(findings, [])


if __name__ == "__main__":
    unittest.main()

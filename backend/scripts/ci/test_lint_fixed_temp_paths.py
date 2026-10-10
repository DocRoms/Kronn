#!/usr/bin/env python3
"""Fixture tests for the fixed-temp-path CI lint (KT-800)."""
import importlib.util
import pathlib
import unittest

_spec = importlib.util.spec_from_file_location(
    "lint_ftp", pathlib.Path(__file__).parent / "lint_fixed_temp_paths.py")
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)

# Fixed paths the lint must refuse (Codex review cases included).
FIXED = [
    'let t = std::env::temp_dir().join("kronn-test-x");\n',
    'let t = std::env::temp_dir().join("kronn-{fixture}");\n',
    'let t = std::env::temp_dir().join(r#"kronn-raw"#);\n',
    'let t = std::env::temp_dir().join(format!("kronn-uuid-cache"));\n',
    'let t = std::env::temp_dir().join(format!("kronn-{}", name /* Uuid planned */));\n',
    'let t = std::env::temp_dir().join(format!("kronn-{}", name)); // process::id() later\n',
    'let t = std::env::temp_dir().join(format!("kronn-process::id()-{}", name));\n',
    'let t = std::env::temp_dir().join(format!(\n    "kronn-{}",\n    name\n));\n',
    'std::fs::create_dir_all("/tmp/kronn-drift-test").ok();\n',
]

# Unique or harmless forms the lint must accept.
ACCEPTED = [
    'let a = std::env::temp_dir().join(format!("k-{}", std::process::id()));\n',
    'let a = std::env::temp_dir().join(format!("kronn-)-{}", std::process::id()));\n',
    'let b = std::env::temp_dir().join(format!(\n    "k-{}",\n    uuid::Uuid::new_v4()\n));\n',
    'let n = std::env::temp_dir().join(format!("k-{}-{}", DIM, nanos));\n',
    'let c = tempfile::tempdir().unwrap();\n',
    'let d = std::env::temp_dir();\n',
    'let e = Some("/tmp/x.png");\n',
    '    // e.g. std::env::temp_dir().join("old-name")\n',
    'let s = "std::env::temp_dir().join(\\"in-a-string\\")";\n',
]


class LintFixedTempPaths(unittest.TestCase):
    def test_fixed_paths_are_refused(self):
        for src in FIXED:
            with self.subTest(src=src):
                self.assertEqual(len(_mod.violations(src)), 1)

    def test_unique_or_harmless_forms_are_accepted(self):
        for src in ACCEPTED:
            with self.subTest(src=src):
                self.assertEqual(_mod.violations(src), [])

    def test_multiline_violation_reports_its_first_line(self):
        self.assertEqual(_mod.violations("\n" + FIXED[7])[0][0], 2)


if __name__ == "__main__":
    unittest.main()

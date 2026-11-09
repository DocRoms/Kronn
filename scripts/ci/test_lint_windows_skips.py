#!/usr/bin/env python3
"""Tests for lint_windows_skips.py (KT-1629). Run: python3 scripts/ci/test_lint_windows_skips.py"""
import tempfile
import unittest
from pathlib import Path

import lint_windows_skips as lint


def scan(source, extra=None):
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "sample.rs"
        path.write_text(source, encoding="utf-8")
        for name, body in (extra or {}).items():
            (Path(tmp) / name).write_text(body, encoding="utf-8")
        return lint.scan(path)


def test_fn(gate, name="a"):
    return f"{gate}\n#[test]\nfn {name}() {{\n}}\n"


class Reasons(unittest.TestCase):
    def test_a_concrete_trailing_reason_is_clean(self):
        findings, unix = scan(test_fn("#[cfg(unix)] // creates symlinks through std::os::unix"))
        self.assertEqual(findings, [])
        self.assertEqual([(n, r) for _, _, n, r in unix], [("a", "creates symlinks through std::os::unix")])

    def test_a_gate_without_a_reason_is_refused(self):
        findings, _ = scan(test_fn("#[cfg(unix)]"))
        self.assertEqual(len(findings), 1)
        self.assertIn("no reason", findings[0])

    def test_a_comment_above_or_a_doc_comment_is_not_the_reason(self):
        for above in ("// POSIX mode bits", "/// POSIX mode bits"):
            findings, _ = scan(f"{above}\n" + test_fn("#[cfg(unix)]"))
            self.assertEqual(len(findings), 1, above)

    def test_the_comment_rustfmt_moves_below_a_long_gate_is_the_reason(self):
        findings, unix = scan("#[cfg(unix)]\n// creates symlinks through std::os::unix\n#[test]\nfn a() {\n}\n")
        self.assertEqual(findings, [])
        self.assertEqual([r for *_, r in unix], ["creates symlinks through std::os::unix"])
        findings, _ = scan("#[cfg(unix)]\n/// creates symlinks\n#[test]\nfn a() {\n}\n")
        self.assertEqual(len(findings), 1)
        findings, _ = scan("#[cfg(unix)]\n// unverified on Windows\n#[test]\nfn a() {\n}\n")
        self.assertIn("not a cause", findings[0])

    def test_unverified_and_its_kin_are_refused(self):
        for reason in (
            "unverified on Windows: gated before the nightly Windows run",
            "not yet verified on Windows",
            "TODO check on windows",
            "flaky on Windows",
            "fails on Windows",
        ):
            findings, _ = scan(test_fn(f"#[cfg(unix)] // {reason}"))
            self.assertEqual(len(findings), 1, reason)
            self.assertIn("not a cause", findings[0])

    def test_a_reason_naming_no_posix_cause_is_refused(self):
        findings, _ = scan(test_fn("#[cfg(unix)] // does not work there"))
        self.assertEqual(len(findings), 1)
        self.assertIn("no concrete POSIX cause", findings[0])

    def test_a_known_windows_defect_is_accepted_when_it_names_its_ticket(self):
        findings, _ = scan(test_fn("#[cfg(unix)] // see KT-1700: the worktree path is case-folded on Windows"))
        self.assertEqual(findings, [])
        findings, _ = scan(test_fn("#[cfg(unix)] // see the ticket"))
        self.assertEqual(len(findings), 1)

    def test_each_concrete_cause_is_accepted(self):
        for reason in (
            "asserts chmod mode bits",
            "relies on POSIX process groups and signals",
            "runs a #!/bin/sh fixture",
            "binds a unix domain socket",
            "runs `sleep` as a program, which Windows lacks",
            "builds non-UTF-8 names from raw bytes",
            "reads /proc/self",
        ):
            findings, _ = scan(test_fn(f"#[cfg(unix)] // {reason}"))
            self.assertEqual(findings, [], reason)


class Forms(unittest.TestCase):
    def test_every_other_way_to_skip_on_windows_is_refused(self):
        for gate in (
            "#[cfg(not(windows))]",
            '#[cfg(not(target_os = "windows"))]',
            '#[cfg(target_family = "unix")]',
            "#[cfg(not(any(windows)))]",
            "#[cfg(any(unix, target_os = \"linux\"))]",
            "#[cfg_attr(windows, ignore)]",
            "#[cfg_attr(not(unix), ignore)]",
            '#[cfg_attr(target_os = "windows", ignore = "no sh")]',
        ):
            findings, _ = scan(test_fn(f"{gate} // creates symlinks through std::os::unix"))
            self.assertGreaterEqual(len(findings), 1, gate)

    def test_the_unix_forms_are_accepted(self):
        for gate in (
            "#[cfg(unix)]",
            "#[cfg(all(unix, feature = \"full-tests\"))]",
            "#[cfg(all(test, unix))]",
            "#[cfg(all(unix, not(target_os = \"macos\")))]",
            '#[cfg(target_os = "macos")]',
            '#[cfg(target_os = "linux")]',
        ):
            findings, _ = scan(test_fn(f"{gate} // creates symlinks through std::os::unix"))
            self.assertEqual(findings, [], gate)

    def test_a_gate_that_still_builds_on_windows_is_not_a_skip(self):
        for gate in ("#[cfg(test)]", "#[cfg(not(unix))]", "#[cfg(windows)]", '#[cfg(feature = "full-tests")]', "#[cfg(any(unix, windows))]"):
            findings, unix = scan(test_fn(gate))
            self.assertEqual((findings, unix), ([], []), gate)

    def test_a_cfg_attr_that_ignores_off_windows_is_not_a_windows_skip(self):
        findings, _ = scan(test_fn("#[cfg_attr(unix, ignore)]"))
        self.assertEqual(findings, [])

    def test_a_tokio_test_with_arguments_and_a_serial_attribute_is_a_test(self):
        source = "#[cfg(unix)]\n#[tokio::test(flavor = \"multi_thread\")]\n#[serial_test::serial(env)]\nasync fn a() {\n}\n"
        findings, _ = scan(source)
        self.assertEqual(len(findings), 1)

    def test_a_gate_on_a_helper_or_production_code_is_not_checked(self):
        for item in ("fn helper() {\n}\n", "use std::os::unix::fs::symlink;\n", "pub struct S;\n", "const X: u8 = 1;\n"):
            findings, _ = scan(f"#[cfg(unix)]\n{item}")
            self.assertEqual(findings, [], item)


class Modules(unittest.TestCase):
    def test_an_inline_module_holding_tests_needs_a_reason(self):
        source = "#[cfg(all(test, unix))]\nmod tests {\n    #[test]\n    fn a() {\n    }\n}\n"
        findings, _ = scan(source)
        self.assertEqual(len(findings), 1)
        self.assertIn("mod tests", findings[0])

    def test_an_inline_module_with_a_reason_is_clean(self):
        source = "#[cfg(all(test, unix))] // creates symlinks through std::os::unix\nmod tests {\n    #[test]\n    fn a() {\n    }\n}\n"
        self.assertEqual(scan(source)[0], [])

    def test_a_module_declared_by_path_is_judged_by_its_file(self):
        gated = "#[cfg(unix)]\n#[path = \"other_tests.rs\"]\nmod other_tests;\n"
        findings, _ = scan(gated, {"other_tests.rs": "#[test]\nfn a() {\n}\n"})
        self.assertEqual(len(findings), 1)
        findings, _ = scan(gated, {"other_tests.rs": "pub fn helper() {\n}\n"})
        self.assertEqual(findings, [])

    def test_a_sibling_file_module_is_judged_by_its_file(self):
        findings, _ = scan("#[cfg(unix)]\nmod checks;\n", {"checks.rs": "#[tokio::test]\nasync fn a() {\n}\n"})
        self.assertEqual(len(findings), 1)

    def test_a_production_module_without_tests_is_not_checked(self):
        findings, _ = scan("#[cfg(unix)]\nmod unix {\n    pub fn open() {\n    }\n}\n")
        self.assertEqual(findings, [])

    def test_a_module_that_builds_on_windows_is_not_checked(self):
        source = "#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() {\n    }\n}\n"
        self.assertEqual(scan(source)[0], [])

    def test_a_whole_file_gate_is_a_module_gate(self):
        findings, _ = scan("#![cfg(unix)]\n#[test]\nfn a() {\n}\n")
        self.assertEqual(len(findings), 1)
        self.assertEqual(scan("#![cfg(unix)] // runs a #!/bin/sh fixture\n#[test]\nfn a() {\n}\n")[0], [])


class RuntimeSkips(unittest.TestCase):
    def test_a_test_that_returns_early_on_windows_is_refused(self):
        for check in ("cfg!(windows)", "!cfg!(unix)", 'cfg!(target_os = "windows")', "cfg!(not(unix))"):
            findings, _ = scan(f"#[test]\nfn a() {{\n    if {check} {{ return; }}\n}}\n")
            self.assertEqual(len(findings), 1, check)
            self.assertIn("at run time", findings[0])

    def test_an_early_return_after_other_statements_is_refused(self):
        findings, _ = scan("#[test]\nfn a() {\n    if cfg!(windows) {\n        eprintln!(\"skipped\");\n        return;\n    }\n}\n")
        self.assertEqual(len(findings), 1)

    def test_an_early_return_in_a_later_test_is_not_blamed_on_this_one(self):
        source = "mod t {\n    #[test]\n    fn a() {\n    }\n\n    #[test]\n    fn b() {\n        if cfg!(windows) { return; }\n    }\n}\n"
        findings, _ = scan(source)
        self.assertEqual([f.split(": ")[1] for f in findings], ["b"])


class ItemBoundary(unittest.TestCase):
    def test_a_closing_brace_that_is_not_at_the_indent_does_not_end_the_test(self):
        # The old end-of-item rule stopped at the first `}` on the test's own
        # indent column; a macro or a raw string can put one earlier.
        source = (
            "#[test]\nfn a() {\n    let s = r#\"\n}\n\"#;\n    if cfg!(windows) { return; }\n}\n"
            "#[test]\nfn b() {\n}\n"
        )
        findings, _ = scan(source)
        self.assertEqual([f.split(": ")[1] for f in findings], ["a"])

    def test_braces_and_attributes_in_strings_and_comments_are_ignored(self):
        source = (
            "// #[cfg(unix)]\n/* #[cfg(unix)] */\nconst S: &str = \"#[cfg(unix)] {\";\n"
            "#[test]\nfn a() {\n    let _ = \"}\";\n    let _ = '}';\n}\n"
            "#[test]\nfn b() {\n    if cfg!(windows) { return; }\n}\n"
        )
        findings, _ = scan(source)
        self.assertEqual([f.split(": ")[1] for f in findings], ["b"])

    def test_a_generic_signature_and_a_where_clause_find_the_body(self):
        source = "#[test]\nfn a<T>() where T: Default {\n    if cfg!(windows) { return; }\n}\n"
        self.assertEqual(len(scan(source)[0]), 1)


class Repository(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scans = [lint.scan(path) for path in lint.rust_files()]

    def test_the_repository_is_clean(self):
        self.assertEqual([f for findings, _ in self.scans for f in findings], [])

    def test_the_repository_inventory_has_no_vague_reason(self):
        for _, unix in self.scans:
            for rel, line, name, reason in unix:
                self.assertFalse(lint.VAGUE.search(reason), f"{rel}:{line}: {name}: {reason}")


if __name__ == "__main__":
    unittest.main()

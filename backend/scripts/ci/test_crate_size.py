#!/usr/bin/env python3
"""Fixture tests for the crate size gate (KT-1854)."""
import contextlib
import importlib.util
import io
import pathlib
import tempfile
import unittest

_spec = importlib.util.spec_from_file_location(
    "crate_size", pathlib.Path(__file__).parent / "crate_size.py")
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)


def write(root, path, text):
    target = pathlib.Path(root) / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text)


def workspace(root, lower_lines):
    write(root, "Cargo.toml",
          '[package]\nname = "top"\n[workspace]\nmembers = [".", "crates/*"]\n')
    write(root, "src/lib.rs", "pub mod a;\n#[cfg(test)]\nmod uptests {\n    mod moved;\n}\n")
    write(root, "src/a.rs", "pub mod inner;\n")
    write(root, "src/a/inner.rs", "fn f() {}\n" * 10)
    write(root, "src/uptests/moved.rs", '#[path = "../shared/moved_test.rs"]\nmod t;\n')
    write(root, "src/shared/moved_test.rs", "fn t() {}\n" * 20)
    write(root, "crates/low/Cargo.toml",
          '[package]\nname = "low"\n[lib]\npath = "../../src/low.rs"\n')
    write(root, "src/low.rs", "pub mod shared {\n    pub mod big;\n}\n")
    write(root, "src/shared/big.rs", "fn g() {}\n" * lower_lines)
    write(root, "src/unused.rs", "fn never_compiled() {}\n" * 5000)


def run(root, limit):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        code = _mod.main([str(root), "--max", str(limit)])
    return code, out.getvalue(), err.getvalue()


class CrateSizeTests(unittest.TestCase):
    def test_each_crate_counts_only_the_files_it_compiles(self):
        with tempfile.TemporaryDirectory() as root:
            workspace(root, 100)
            sizes = dict(_mod.measure(root))
        # top: lib.rs + a.rs + a/inner.rs + the wrapper + the test file it names.
        self.assertEqual(sizes["top"], 6 + 2 + 11 + 3 + 21)
        # low: its root and the module declared in an inline block.
        self.assertEqual(sizes["low"], 4 + 101)

    def test_a_crate_over_the_limit_fails_and_is_named(self):
        with tempfile.TemporaryDirectory() as root:
            workspace(root, 400)
            code, out, err = run(root, 300)
        self.assertEqual(code, 1)
        self.assertIn("low", [l.split()[0] for l in out.splitlines() if "over" in l])
        self.assertIn("1 crate(s) over 300 lines", err)

    def test_every_crate_under_the_limit_passes(self):
        with tempfile.TemporaryDirectory() as root:
            workspace(root, 100)
            code, _, err = run(root, 300)
        self.assertEqual((code, err), (0, ""))

    def test_the_default_limit_is_the_documented_one(self):
        self.assertEqual(_mod.MAX_LINES, 200_000)


if __name__ == "__main__":
    unittest.main()

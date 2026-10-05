#!/usr/bin/env python3
"""Fixture tests for the spawn-environment gate (lint_child_env.py)."""
import importlib.util
import pathlib
import tempfile
import unittest

_spec = importlib.util.spec_from_file_location(
    "lint_child_env", pathlib.Path(__file__).parent / "lint_child_env.py")
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)


def verdicts(src):
    return [(name, verdict) for _, name, verdict in _mod.call_sites(src, "fixture.rs")]


class LintChildEnv(unittest.TestCase):
    def test_a_bare_spawn_is_flagged(self):
        src = 'fn run() {\n    let _ = async_cmd("docker").output();\n}\n'
        self.assertEqual(verdicts(src), [("run", "bare")])

    def test_literal_git_is_covered_by_the_git_policy(self):
        src = 'fn run() {\n    sync_cmd("git").arg("status");\n    sync_cmd( "gitk" );\n}\n'
        self.assertEqual(verdicts(src), [("run", "git"), ("run", "bare")])

    def test_child_env_must_be_in_the_same_function(self):
        src = (
            "fn isolated() {\n"
            '    let mut c = sync_cmd("gh");\n'
            "    crate::core::child_env::isolate(&mut c, ChildRoute::GitHost);\n"
            "}\n"
            "fn other() {\n"
            '    let c = sync_cmd("gh");\n'
            "}\n"
        )
        self.assertEqual(verdicts(src), [("isolated", "isolated"), ("other", "bare")])

    def test_child_env_in_a_comment_or_string_does_not_count(self):
        src = (
            "fn run() {\n"
            "    // child_env::isolate is applied elsewhere\n"
            '    let note = "child_env::isolate";\n'
            '    let c = async_cmd(program);\n'
            "}\n"
        )
        self.assertEqual(verdicts(src), [("run", "bare")])

    def test_nested_function_is_its_own_scope(self):
        src = (
            "fn outer() {\n"
            "    child_env::isolate(&mut x, ChildRoute::Tool);\n"
            "    fn inner() { let _ = sync_cmd(\"wsl.exe\"); }\n"
            "}\n"
        )
        self.assertEqual(verdicts(src), [("inner", "bare")])

    def test_test_items_are_exempt_but_not_what_follows_them(self):
        src = (
            "#[cfg(test)]\n"
            '#[path = "x_test.rs"]\n'
            "mod x_test;\n"
            "fn prod() { let _ = sync_cmd(\"kill\"); }\n"
            "#[test]\n"
            "fn t() { let _ = sync_cmd(\"kill\"); }\n"
            "#[tokio::test]\n"
            "async fn t2() { let _ = async_cmd(\"kill\"); }\n"
            "#[cfg(all(test, unix))]\n"
            "mod tests {\n"
            "    fn helper() { let _ = sync_cmd(\"sh\"); }\n"
            "}\n"
            "fn after() { let _ = sync_cmd(\"kill\"); }\n"
        )
        self.assertEqual(verdicts(src), [("prod", "bare"), ("after", "bare")])

    def test_exceptions_and_stale_entries(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "core").mkdir()
            (root / "core" / "probe.rs").write_text(
                'fn declared() { let _ = async_cmd("x"); }\n'
                'fn undeclared() { let _ = async_cmd("y"); }\n'
            )
            (root / "core" / "probe_test.rs").write_text(
                'fn ignored() { let _ = async_cmd("z"); }\n'
            )
            saved = _mod.EXCEPTIONS
            _mod.EXCEPTIONS = {
                ("core/probe.rs", "declared"): "fixture",
                ("core/gone.rs", "removed"): "fixture",
            }
            try:
                violations, stale = _mod.check(root)
            finally:
                _mod.EXCEPTIONS = saved
        self.assertEqual(violations, ["src/core/probe.rs:2 in fn undeclared"])
        self.assertEqual(stale, ["src/core/gone.rs fn removed"])

    def test_the_backend_tree_passes(self):
        src = pathlib.Path(__file__).resolve().parents[2] / "src"
        violations, stale = _mod.check(src)
        self.assertEqual((violations, stale), ([], []))


if __name__ == "__main__":
    unittest.main()

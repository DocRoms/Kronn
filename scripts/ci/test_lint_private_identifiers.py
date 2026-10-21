#!/usr/bin/env python3
"""Fixture tests for the private-identifier CI lint (KT-1413).

They run against a synthetic deny-list (digests of made-up tokens), so no real
private name appears here either."""
import importlib.util
import io
import pathlib
import contextlib
import tempfile
import unittest

_spec = importlib.util.spec_from_file_location(
    "lint_pi", pathlib.Path(__file__).parent / "lint_private_identifiers.py")
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)

WORD = "zzword"
PATH = "example/secret-dir"
DENY = frozenset({_mod.digest(WORD), _mod.digest(PATH)})

REFUSED = [
    f"// {WORD} step 10: the key\n",
    f"// {WORD.capitalize()}'s glossary\n",
    f"let p = \"front_{WORD}/plugins\";\n",
    f"let p = \"front-{WORD}\";\n",
    f"const {WORD.upper()}_PACKAGE: &str = \"\";\n",
    f"fn {WORD}_matrix() {{}}\n",
    f"[src: file: {PATH}:1]\n",
    f"let key = \"{PATH}/id\";\n",
    f"see `{WORD}`, then `{PATH}`.\n",
]

ACCEPTED = [
    "// an audit matrix repository, a Node service, a PHP site\n",
    "[src: file: .ssh/deploy-key]\n",
    "[src: file: keys/service-key:exists]\n",
    "let key = \".ssh/id_ed25519\";\n",
    f"let glued = \"x{WORD}y\";\n",  # glued to letters: not the bare word
    "// other dirs: example/other-dir, secret-dir\n",
]


class LintPrivateIdentifiers(unittest.TestCase):
    def test_denied_tokens_are_refused(self):
        for src in REFUSED:
            with self.subTest(src=src):
                self.assertGreaterEqual(len(_mod.violations(src, DENY)), 1)

    def test_neutral_text_is_accepted(self):
        for src in ACCEPTED:
            with self.subTest(src=src):
                self.assertEqual(_mod.violations(src, DENY), [])

    def test_reports_the_line_and_the_digest_only(self):
        found = _mod.violations("ok\nok\n" + REFUSED[0], DENY)
        self.assertEqual(found, [(3, _mod.digest(WORD))])

    def test_scan_fails_on_a_reintroduced_identifier(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / "clean.rs").write_text("let k = \".ssh/deploy-key\";\n", encoding="utf-8")
            self.assertEqual(_mod.scan(root, DENY), [])
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(_mod.main(root, DENY), 0)
            (root / "dirty.rs").write_text(f"let k = \"{PATH}\";\n", encoding="utf-8")
            found = _mod.scan(root, DENY)
            self.assertEqual(len(found), 1)
            self.assertTrue(found[0].startswith("dirty.rs:1: private identifier"))
            self.assertNotIn(WORD, found[0])
            self.assertNotIn(PATH, found[0])
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                self.assertEqual(_mod.main(root, DENY), 1)
            self.assertNotIn(PATH, out.getvalue())

    def test_real_deny_list_is_hashes_only(self):
        self.assertTrue(_mod.DENY_DIGESTS)
        for d in _mod.DENY_DIGESTS:
            self.assertRegex(d, r"^[0-9a-f]{64}$")

    def test_the_repository_tree_is_clean(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        self.assertEqual(_mod.scan(root), [])


if __name__ == "__main__":
    unittest.main()

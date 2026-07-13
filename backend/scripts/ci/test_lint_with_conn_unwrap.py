#!/usr/bin/env python3
"""Fixture tests for the with_conn-unwrap CI lint (Codex review: the
4k-char scan cap silently skipped long closures)."""
import importlib.util
import pathlib
import unittest

_spec = importlib.util.spec_from_file_location(
    "lint_wcu", pathlib.Path(__file__).parent / "lint_with_conn_unwrap.py")
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)


class LintWithConnUnwrap(unittest.TestCase):
    def test_flags_unwrap_placed_beyond_4k_chars(self):
        # The exact blind spot: a closure body longer than the old cap.
        filler = '            let _x = "' + "a" * 4200 + '";\n'
        src = (
            "fn f(db: &Db) {\n"
            "    db.with_conn(move |conn| {\n"
            + filler +
            "        conn.query_row(q, [], |r| r.get(0)).unwrap();\n"
            "        Ok(())\n"
            "    });\n"
            "}\n"
        )
        viol = _mod.find_violations(src, "long.rs")
        self.assertEqual(len(viol), 1, "unwrap after 4k chars MUST be flagged")

    def test_clean_closure_and_test_module_are_exempt(self):
        src = (
            "fn f(db: &Db) {\n"
            "    db.with_conn(|conn| Ok(conn.count()?));\n"
            "}\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    fn t(db: &Db) { db.with_conn(|c| Ok(c.count().unwrap())); }\n"
            "}\n"
        )
        self.assertEqual(_mod.find_violations(src, "clean.rs"), [])

    def test_unwrap_outside_the_closure_is_not_flagged(self):
        src = (
            "fn f(db: &Db) {\n"
            "    let v = db.with_conn(|conn| Ok(conn.count()?));\n"
            "    v.unwrap();\n"  # caller-side unwrap: out of this lint's scope
            "}\n"
        )
        self.assertEqual(_mod.find_violations(src, "outside.rs"), [])

    def test_literal_paren_in_string_does_not_hide_the_unwrap(self):
        # Codex finding 3 — exact repro: a ")" string literal closed the
        # scan artificially and the unwrap after it was invisible.
        src = (
            "fn f(db: &Db) {\n"
            "    db.with_conn(|conn| {\n"
            '        let text = ")";\n'
            "        conn.query_row(q, [], |r| r.get(0)).unwrap();\n"
            "        Ok(())\n"
            "    });\n"
            "}\n"
        )
        self.assertEqual(len(_mod.find_violations(src, "lit.rs")), 1)

    def test_parens_in_comments_raw_strings_and_chars_are_ignored(self):
        src = (
            "fn f(db: &Db) {\n"
            "    db.with_conn(|conn| {\n"
            "        // closing ) in a comment\n"
            "        /* nested /* )) */ still ) comment */\n"
            '        let raw = r#"raw ) text"#;\n'
            "        let ch = ')';\n"
            "        let lt: &'static str = x;\n"
            "        conn.count().unwrap();\n"
            "        Ok(())\n"
            "    });\n"
            "}\n"
        )
        self.assertEqual(len(_mod.find_violations(src, "mix.rs")), 1,
                         "the real unwrap is still flagged, literal parens ignored")

    def test_unwrap_text_inside_a_string_is_not_flagged(self):
        src = (
            "fn f(db: &Db) {\n"
            '    db.with_conn(|conn| { let s = ".unwrap()"; Ok(s.len()) });\n'
            "}\n"
        )
        self.assertEqual(_mod.find_violations(src, "strunwrap.rs"), [])

    def test_unbalanced_call_fails_loudly(self):
        with self.assertRaises(RuntimeError):
            _mod.find_violations("db.with_conn(|c| {", "broken.rs")


if __name__ == "__main__":
    unittest.main()

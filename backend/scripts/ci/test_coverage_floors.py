#!/usr/bin/env python3
"""Tests for the backend coverage floors (KT-1118)."""

from __future__ import annotations

import contextlib
import copy
import importlib.util
import io
import json
import pathlib
import tempfile
import unittest

_SPEC = importlib.util.spec_from_file_location(
    "coverage_floors", pathlib.Path(__file__).with_name("coverage_floors.py")
)
cf = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(cf)

ROOT = "/home/runner/work/Kronn/Kronn/backend/"


def metric(covered: int, count: int) -> dict:
    return {"count": count, "covered": covered, "percent": 100.0 * covered / count}


def summary(lines=(99, 100), functions=(99, 100), regions=(99, 100)) -> dict:
    return {"lines": metric(*lines), "functions": metric(*functions), "regions": metric(*regions)}


def export(totals: dict | None = None, files: dict | None = None) -> dict:
    files = files if files is not None else {suffix: summary() for suffix in cf.KEY_FILE_FLOORS}
    return {
        "type": "llvm.coverage.json.export",
        "data": [{
            "totals": totals or summary((900, 1000), (900, 1000), (900, 1000)),
            "files": [{"filename": ROOT + suffix, "summary": s} for suffix, s in files.items()],
        }],
    }


class CheckTest(unittest.TestCase):
    def test_floors_met(self):
        _, failures = cf.check(export())
        self.assertEqual(failures, [])

    def test_global_floor_per_metric(self):
        for name in cf.METRICS:
            totals = summary((900, 1000), (900, 1000), (900, 1000))
            totals[name] = metric(829, 1000)
            _, failures = cf.check(export(totals))
            self.assertEqual(len(failures), 1, name)
            self.assertIn(f"total {name}", failures[0])

    def test_exactly_at_floor_passes(self):
        totals = summary((83, 100), (83, 100), (83, 100))
        self.assertEqual(cf.check(export(totals))[1], [])

    def test_key_file_under_floor(self):
        files = {suffix: summary() for suffix in cf.KEY_FILE_FLOORS}
        files["src/core/crypto.rs"] = summary(regions=(98, 100))
        _, failures = cf.check(export(files=files))
        self.assertEqual(len(failures), 1)
        self.assertIn("src/core/crypto.rs", failures[0])

    def test_missing_key_file_fails(self):
        files = {suffix: summary() for suffix in cf.KEY_FILE_FLOORS}
        del files["src/db/mcps.rs"]
        _, failures = cf.check(export(files=files))
        self.assertEqual(len(failures), 1)
        self.assertIn("found none", failures[0])

    def test_ambiguous_or_partial_name_fails(self):
        files = {suffix: summary() for suffix in cf.KEY_FILE_FLOORS}
        files["src/mcp/src/core/config.rs"] = summary()
        self.assertEqual(len(cf.check(export(files=files))[1]), 1)
        with self.assertRaises(cf.CoverageError):
            cf.match_file([{"filename": ROOT + "src/core/xcrypto.rs"}], "src/core/crypto.rs")

    def test_malformed_export_fails(self):
        for bad in [{}, {"data": []}, {"data": [{}, {}]}, {"data": [{"totals": {}}]}]:
            with self.assertRaises(cf.CoverageError, msg=bad):
                cf.check(bad)

    def test_cli_has_no_fallback(self):
        with tempfile.TemporaryDirectory() as tmp, contextlib.redirect_stdout(io.StringIO()):
            missing = pathlib.Path(tmp) / "absent.json"
            self.assertEqual(cf.main(["check", str(missing)]), 1)
            good = pathlib.Path(tmp) / "cov.json"
            good.write_text(json.dumps(export()))
            out = pathlib.Path(tmp) / "summary.json"
            self.assertEqual(cf.main(["check", str(good), "--summary-out", str(out)]), 0)
            self.assertEqual(json.loads(out.read_text())["totals"]["lines"], {"count": 1000, "covered": 900})


class CompareTest(unittest.TestCase):
    def test_same_counts_pass(self):
        self.assertEqual(cf.compare(cf.summarize(export()), cf.summarize(export())), [])

    def test_any_count_difference_fails(self):
        base = export()
        other = copy.deepcopy(base)
        other["data"][0]["files"][0]["summary"]["regions"] = metric(98, 100)
        other["data"][0]["totals"]["regions"] = metric(899, 1000)
        differences = cf.compare(cf.summarize(base), cf.summarize(other))
        self.assertEqual(len(differences), 2)
        self.assertTrue(differences[0].startswith("totals differ"))

    def test_file_only_in_one_run_fails(self):
        files = {suffix: summary() for suffix in cf.KEY_FILE_FLOORS}
        fewer = dict(files)
        del fewer["src/core/crypto.rs"]
        differences = cf.compare(cf.summarize(export(files=files)), cf.summarize(export(files=fewer)))
        self.assertEqual(differences, [f"{ROOT}src/core/crypto.rs: only in the expected run"])


if __name__ == "__main__":
    unittest.main()

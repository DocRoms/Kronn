#!/usr/bin/env python3
"""Tests for the partitioned backend run checks (KT-1118)."""

from __future__ import annotations

import contextlib
import importlib.util
import io
import json
import pathlib
import tempfile
import unittest

_SPEC = importlib.util.spec_from_file_location(
    "backend_partitions", pathlib.Path(__file__).with_name("backend_partitions.py")
)
bp = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(bp)

SHA = "a" * 40
TESTS = {
    "kronn": ["core::crypto::tests::roundtrip", "core::config::tests::save", "db::tests::open"],
    "kronn::it": ["api_tests::health", "api_tests::setup"],
}


def nextest_list(tests: dict[str, list[str]], ignored: dict[str, list[str]] | None = None) -> dict:
    suites = {}
    for binary_id, names in tests.items():
        cases = {name: {"ignored": False, "filter-match": {"status": "matches"}} for name in names}
        for name in (ignored or {}).get(binary_id, []):
            cases[name] = {"ignored": True, "filter-match": {"status": "mismatch", "reason": "ignored"}}
        suites[binary_id] = {"binary-id": binary_id, "testcases": cases}
    return {"rust-build-meta": {}, "test-count": 0, "rust-suites": suites}


def junit(cases: list[tuple[str, str]], failed: set[str] = frozenset()) -> str:
    by_suite: dict[str, list[str]] = {}
    for binary_id, name in cases:
        by_suite.setdefault(binary_id, []).append(name)
    body = []
    for binary_id, names in by_suite.items():
        rows = []
        for name in names:
            inner = '<failure type="test failure"/>' if name in failed else ""
            rows.append(f'<testcase name="{name}" classname="{binary_id}" time="0.1">{inner}</testcase>')
        body.append(f'<testsuite name="{binary_id}" tests="{len(names)}">{"".join(rows)}</testsuite>')
    return f'<?xml version="1.0"?><testsuites name="nextest-run">{"".join(body)}</testsuites>'


def all_cases() -> list[tuple[str, str]]:
    return [(binary_id, name) for binary_id, names in TESTS.items() for name in names]


class Workspace:
    """An archive plus the artifacts of COUNT partitions, splitting the tests round-robin."""

    def __init__(self, root: pathlib.Path, count: int = 2):
        self.root = root
        self.count = count
        self.archive = root / "kronn-tests.tar.zst"
        self.archive.write_bytes(b"archive bytes")
        self.artifacts = root / "artifacts"
        self.inventory = root / "inventory.json"
        self.inventory.write_text(json.dumps(nextest_list(TESTS, {"kronn": ["core::slow_ignored"]})))
        cases = all_cases()
        for partition in range(1, count + 1):
            self.write_partition(partition, cases[partition - 1::count])

    def partition_dir(self, partition: int) -> pathlib.Path:
        return self.artifacts / f"backend-partition-{partition}"

    def write_partition(self, partition: int, cases, failed=frozenset(), profiles=("kronn-123_0.profraw",)):
        directory = self.partition_dir(partition)
        (directory / "profiles").mkdir(parents=True, exist_ok=True)
        (directory / "junit.xml").write_text(junit(cases, failed))
        for profile in profiles:
            (directory / "profiles" / profile).write_bytes(f"profile {partition}".encode())
        bp.write_manifest(partition, self.count, self.archive, SHA, directory)

    def verify(self) -> list[str]:
        expected = bp.inventory(json.loads(self.inventory.read_text()))
        return bp.verify(self.count, self.artifacts, expected, bp.sha256(self.archive), SHA)


class PlanTest(unittest.TestCase):
    def test_plan_lists_every_partition(self):
        self.assertEqual(bp.plan(3), "matrix=[1, 2, 3]\ncount=3\n")

    def test_count_bounds(self):
        self.assertEqual(bp.partition_count("1"), 1)
        for raw in ["0", "17", "two", ""]:
            with self.assertRaises(bp.PartitionError, msg=raw):
                bp.partition_count(raw)


class InventoryTest(unittest.TestCase):
    def test_only_matching_tests_count(self):
        found = bp.inventory(nextest_list(TESTS, {"kronn": ["core::slow_ignored"]}))
        self.assertEqual(len(found), 5)
        self.assertIn("kronn::it api_tests::health", found)
        self.assertNotIn("kronn core::slow_ignored", found)

    def test_empty_list_is_an_error(self):
        with self.assertRaises(bp.PartitionError):
            bp.inventory({"rust-suites": {}})
        with self.assertRaises(bp.PartitionError):
            bp.inventory(nextest_list({}, {"kronn": ["only_ignored"]}))


class VerifyTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.ws = Workspace(pathlib.Path(self._tmp.name))

    def tearDown(self):
        self._tmp.cleanup()

    def test_complete_run_passes(self):
        self.assertEqual(self.ws.verify(), [])

    def test_missing_partition_artifact_fails(self):
        for path in sorted(self.ws.partition_dir(2).rglob("*"), reverse=True):
            path.unlink() if path.is_file() else path.rmdir()
        self.ws.partition_dir(2).rmdir()
        problems = self.ws.verify()
        self.assertIn("backend-partition-2: artifact missing", problems)
        self.assertTrue(any("without a result" in p for p in problems), problems)

    def test_missing_manifest_fails(self):
        (self.ws.partition_dir(1) / "manifest.json").unlink()
        self.assertTrue(any("unreadable manifest.json" in p for p in self.ws.verify()))

    def test_missing_junit_fails(self):
        (self.ws.partition_dir(1) / "junit.xml").unlink()
        self.assertTrue(any("unreadable junit.xml" in p for p in self.ws.verify()))

    def test_missing_profile_fails(self):
        (self.ws.partition_dir(2) / "profiles" / "kronn-123_0.profraw").unlink()
        self.assertIn("backend-partition-2: profile kronn-123_0.profraw missing", self.ws.verify())

    def test_inventory_test_never_run_fails(self):
        cases = all_cases()[0::2]
        self.ws.write_partition(1, cases[1:])
        self.assertIn(f"inventory test never ran: {cases[0][0]} {cases[0][1]}", self.ws.verify())

    def test_test_outside_inventory_fails(self):
        self.ws.write_partition(1, all_cases()[0::2] + [("kronn", "new::test")])
        self.assertIn("ran a test outside the inventory: kronn new::test", self.ws.verify())

    def test_test_run_twice_fails(self):
        self.ws.write_partition(1, all_cases())
        self.assertTrue(any("also ran in partition" in p for p in self.ws.verify()))

    def test_failed_test_fails(self):
        cases = all_cases()[0::2]
        self.ws.write_partition(1, cases, failed={cases[0][1]})
        self.assertIn(f"backend-partition-1: failed {cases[0][0]} {cases[0][1]}", self.ws.verify())

    def test_other_archive_or_commit_fails(self):
        self.ws.archive.write_bytes(b"rebuilt archive")
        self.assertEqual(sum("archive_sha256" in p for p in self.ws.verify()), 2)
        manifest = self.ws.partition_dir(1) / "manifest.json"
        data = json.loads(manifest.read_text())
        data["sha"] = "b" * 40
        manifest.write_text(json.dumps(data))
        self.assertTrue(any("backend-partition-1: sha" in p for p in self.ws.verify()))

    def test_partition_from_another_plan_fails(self):
        self.ws.partition_dir(3).mkdir(parents=True)
        problems = self.ws.verify()
        self.assertIn("backend-partition-3: not one of the 2 planned partitions", problems)

    def test_cli_exit_codes(self):
        argv = ["verify", "--count", "2", "--artifacts", str(self.ws.artifacts), "--inventory",
                str(self.ws.inventory), "--archive", str(self.ws.archive), "--sha", SHA]
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(bp.main(argv), 0)
            (self.ws.partition_dir(2) / "junit.xml").unlink()
            self.assertEqual(bp.main(argv), 1)
            self.assertEqual(bp.main(["plan", "--count", "0"]), 1)


class ManifestTest(unittest.TestCase):
    def test_manifest_requires_junit_and_profiles(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            archive = root / "a.tar.zst"
            archive.write_bytes(b"x")
            with self.assertRaisesRegex(bp.PartitionError, "junit.xml"):
                bp.write_manifest(1, 2, archive, SHA, root)
            (root / "junit.xml").write_text(junit([]))
            with self.assertRaisesRegex(bp.PartitionError, "profraw"):
                bp.write_manifest(1, 2, archive, SHA, root)
            with self.assertRaisesRegex(bp.PartitionError, "outside"):
                bp.write_manifest(3, 2, archive, SHA, root)


class StageTest(unittest.TestCase):
    def test_profiles_with_equal_names_do_not_collide(self):
        with tempfile.TemporaryDirectory() as tmp:
            ws = Workspace(pathlib.Path(tmp))
            dest = pathlib.Path(tmp) / "llvm-cov-target"
            self.assertEqual(bp.stage(2, ws.artifacts, dest), 2)
            staged = sorted(p.name for p in dest.iterdir())
            self.assertEqual(staged, ["p1-kronn-123_0.profraw", "p2-kronn-123_0.profraw"])
            self.assertEqual((dest / "p2-kronn-123_0.profraw").read_bytes(), b"profile 2")
            with self.assertRaisesRegex(bp.PartitionError, "refusing to mix"):
                bp.stage(2, ws.artifacts, dest)

    def test_nothing_to_stage_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaisesRegex(bp.PartitionError, "no profile"):
                bp.stage(2, pathlib.Path(tmp), pathlib.Path(tmp) / "dest")


if __name__ == "__main__":
    unittest.main()

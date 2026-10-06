#!/usr/bin/env python3
"""Tests for the tech-debt registry check (KT-1061)."""

from __future__ import annotations

import importlib.util
import pathlib
import unittest

_SPEC = importlib.util.spec_from_file_location(
    "tech_debt_registry", pathlib.Path(__file__).with_name("tech_debt_registry.py")
)
registry = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(registry)

INDEX = """
| ID | Problem | Area | Severity |
|----|---------|------|----------|
| TD-20260101-open-item | Something is wrong. | Backend | Medium |
"""
OPEN = "- **ID**: TD-20260101-open-item\n- **Status**: OPEN\n"


class TechDebtRegistryTest(unittest.TestCase):
    def test_matching_index_and_files_pass(self):
        self.assertEqual(registry.problems(INDEX, {"TD-20260101-open-item": OPEN}), [])

    def test_row_without_detail_file(self):
        found = registry.problems(INDEX, {})
        self.assertEqual(len(found), 1)
        self.assertIn("row without", found[0])

    def test_orphan_detail_file(self):
        details = {"TD-20260101-open-item": OPEN, "TD-20260202-orphan": OPEN}
        found = registry.problems(INDEX, details)
        self.assertEqual(found, ["docs/tech-debt/TD-20260202-orphan.md: no row in the index"])

    def test_closed_row(self):
        index = INDEX.replace("| Medium |", "| Closed |")
        found = registry.problems(index, {"TD-20260101-open-item": OPEN})
        self.assertEqual(len(found), 1)
        self.assertIn("closed row", found[0])

    def test_closed_problem_text(self):
        index = INDEX.replace("Something is wrong.", "**Closed by KT-1.** Done.")
        self.assertEqual(len(registry.problems(index, {"TD-20260101-open-item": OPEN})), 1)

    def test_fixed_status_in_detail_file(self):
        for status in ["🟢 Fixed 2026-09-15", "RESOLVED — shipped", "**Resolved 2026-08-30**"]:
            body = f"- **Status**: {status}\n"
            found = registry.problems(INDEX, {"TD-20260101-open-item": body})
            self.assertEqual(len(found), 1, status)

    def test_partial_status_stays_open(self):
        body = "- **Status**: partly resolved by KT-953; still open.\n"
        self.assertEqual(registry.problems(INDEX, {"TD-20260101-open-item": body}), [])

    def test_repository_registry_is_consistent(self):
        self.assertEqual(registry.main(), 0)


if __name__ == "__main__":
    unittest.main()

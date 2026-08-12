#!/usr/bin/env python3
"""Tests for the Context Budget Gate.

A gate that cannot fail is not a gate. These cases pin the two behaviours that
matter: it refuses growth, and it does not confuse "over ceiling" (a failure)
with "above target" (recorded debt). Both are checked on a synthetic repo root
so the real bootstrap files are never touched.
"""

from __future__ import annotations

import importlib.util
import pathlib
import sys
import tempfile
import unittest

_SPEC = importlib.util.spec_from_file_location(
    "context_budget", pathlib.Path(__file__).with_name("context_budget.py")
)
assert _SPEC and _SPEC.loader
context_budget = importlib.util.module_from_spec(_SPEC)
# Register before exec: dataclass/annotation resolution looks the module up in
# sys.modules on Python >= 3.13.
sys.modules[_SPEC.name] = context_budget
_SPEC.loader.exec_module(context_budget)


class ContextBudgetGateTest(unittest.TestCase):
    def _root(self, sizes: dict[str, int]) -> pathlib.Path:
        root = pathlib.Path(tempfile.mkdtemp())
        for rel, size in sizes.items():
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b"x" * size)
        return root

    def _budgets(self, ceiling: int, target: int) -> list[dict[str, object]]:
        return [
            {
                "path": "docs/AGENTS.md",
                "max_bytes": ceiling,
                "target_bytes": target,
                "why": "test",
            }
        ]

    def test_passes_at_ceiling(self):
        """Exactly at the ceiling is allowed — the ratchet forbids growth, not parity."""
        root = self._root({"docs/AGENTS.md": 1000})
        context_budget.BUDGETS = self._budgets(1000, 1000)
        context_budget.AGGREGATE_MAX_BYTES = 1000
        context_budget.AGGREGATE_TARGET_BYTES = 1000
        self.assertEqual(context_budget.check(root), 0)

    def test_fails_when_a_file_grows_by_one_byte(self):
        """The whole point: growth is refused, however small."""
        root = self._root({"docs/AGENTS.md": 1001})
        context_budget.BUDGETS = self._budgets(1000, 1000)
        context_budget.AGGREGATE_MAX_BYTES = 10_000
        context_budget.AGGREGATE_TARGET_BYTES = 1000
        self.assertEqual(context_budget.check(root), 1)

    def test_debt_above_target_is_not_a_failure(self):
        """Above target but under ceiling is recorded debt, not a red build —
        otherwise the gate could never be introduced on an oversized file."""
        root = self._root({"docs/AGENTS.md": 900})
        context_budget.BUDGETS = self._budgets(1000, 500)
        context_budget.AGGREGATE_MAX_BYTES = 10_000
        context_budget.AGGREGATE_TARGET_BYTES = 500
        self.assertEqual(context_budget.check(root), 0)

    def test_aggregate_can_fail_even_when_each_file_passes(self):
        """Splitting a bloated file into two compliant ones must not buy a pass."""
        root = self._root({"docs/AGENTS.md": 900, "CLAUDE.md": 900})
        context_budget.BUDGETS = [
            {"path": "docs/AGENTS.md", "max_bytes": 1000, "target_bytes": 1000, "why": "t"},
            {"path": "CLAUDE.md", "max_bytes": 1000, "target_bytes": 1000, "why": "t"},
        ]
        context_budget.AGGREGATE_MAX_BYTES = 1000
        context_budget.AGGREGATE_TARGET_BYTES = 1000
        self.assertEqual(context_budget.check(root), 1)

    def test_missing_bootstrap_file_fails(self):
        """A dangling bootstrap reference sends agents to a dead file."""
        root = self._root({})
        context_budget.BUDGETS = self._budgets(1000, 1000)
        context_budget.AGGREGATE_MAX_BYTES = 10_000
        context_budget.AGGREGATE_TARGET_BYTES = 1000
        self.assertEqual(context_budget.check(root), 1)

    def _dup_root(self, shared: str) -> pathlib.Path:
        """A repo where the same paragraph sits in the bootstrap AND its canonical home."""
        root = pathlib.Path(tempfile.mkdtemp())
        (root / "docs").mkdir()
        (root / "docs/AGENTS.md").write_text(f"# Boot\n\n{shared}\n")
        (root / "docs/stack.md").write_text(f"# Stack\n\n{shared}\n")
        return root

    def test_content_reappearing_in_the_bootstrap_fails(self):
        """The split being undone is the failure mode that let 84 KiB accumulate."""
        shared = "Backend uses axum and rusqlite. " * 12  # well over the threshold
        context_budget.BUDGETS = self._budgets(100_000, 100_000)
        context_budget.AGGREGATE_MAX_BYTES = 100_000
        context_budget.AGGREGATE_TARGET_BYTES = 100_000
        context_budget.CANONICAL_DESTINATIONS = ["docs/stack.md"]
        context_budget.EXCEPTIONS = []
        self.assertEqual(context_budget.check(self._dup_root(shared)), 1)

    def test_short_shared_text_is_not_flagged(self):
        """Pointers and headings legitimately appear in both files."""
        context_budget.BUDGETS = self._budgets(100_000, 100_000)
        context_budget.AGGREGATE_MAX_BYTES = 100_000
        context_budget.AGGREGATE_TARGET_BYTES = 100_000
        context_budget.CANONICAL_DESTINATIONS = ["docs/stack.md"]
        context_budget.EXCEPTIONS = []
        self.assertEqual(context_budget.check(self._dup_root("See docs/stack.md")), 0)

    def test_live_exception_waives_a_duplicate(self):
        shared = "Backend uses axum and rusqlite. " * 12
        context_budget.BUDGETS = self._budgets(100_000, 100_000)
        context_budget.AGGREGATE_MAX_BYTES = 100_000
        context_budget.AGGREGATE_TARGET_BYTES = 100_000
        context_budget.CANONICAL_DESTINATIONS = ["docs/stack.md"]
        context_budget.EXCEPTIONS = [{
            "paragraph_startswith": "Backend uses axum",
            "reason": "test",
            "expires": "2999-01-01",
        }]
        self.assertEqual(context_budget.check(self._dup_root(shared)), 0)

    def test_expired_exception_fails_even_with_nothing_duplicated(self):
        """A stale waiver means nobody re-read it — and the next one gets trusted
        blindly. It must break the build on its own."""
        root = pathlib.Path(tempfile.mkdtemp())
        (root / "docs").mkdir()
        (root / "docs/AGENTS.md").write_text("# Boot\n\nnothing shared\n")
        context_budget.BUDGETS = self._budgets(100_000, 100_000)
        context_budget.AGGREGATE_MAX_BYTES = 100_000
        context_budget.AGGREGATE_TARGET_BYTES = 100_000
        context_budget.CANONICAL_DESTINATIONS = []
        context_budget.EXCEPTIONS = [{
            "paragraph_startswith": "anything",
            "reason": "test",
            "expires": "2020-01-01",
        }]
        self.assertEqual(context_budget.check(root), 1)

    def test_sections_are_reported_with_the_preamble(self):
        got = context_budget.sections("intro text\n\n## A\n\nbody\n\n## B\n\nmore\n")
        self.assertEqual([h for h, _ in got], ["(preamble)", "## A", "## B"])

    def test_real_repo_is_within_its_own_ceilings(self):
        """Guards against committing a ratchet the repo already violates."""
        importlib.reload(context_budget)
        root = pathlib.Path(__file__).resolve().parents[3]
        self.assertEqual(context_budget.check(root), 0)


if __name__ == "__main__":
    unittest.main(verbosity=2)

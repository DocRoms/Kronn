#!/usr/bin/env python3
"""Tests for the MCP surface budget gate — KT-192.

A budget gate that cannot fail is decoration. Each test here breaks one thing and
asserts the gate notices, because that is the only property that matters: it must
bite when the surface grows, not merely print numbers.

Run: python3 backend/scripts/ci/test_mcp_surface_budget.py
"""

from __future__ import annotations

import io
import pathlib
import sys
import unittest
from contextlib import redirect_stderr, redirect_stdout

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import mcp_surface_budget as gate  # noqa: E402

REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]


def run() -> int:
    """Run the gate, swallowing its output."""
    with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
        return gate.check(REPO_ROOT)


def run_capturing_stderr() -> tuple[int, str]:
    err = io.StringIO()
    with redirect_stdout(io.StringIO()), redirect_stderr(err):
        code = gate.check(REPO_ROOT)
    return code, err.getvalue()


class McpSurfaceBudgetTests(unittest.TestCase):
    def setUp(self) -> None:
        self._catalogue = gate.CATALOGUE_MAX_BYTES
        self._declaration = gate.DECLARATION_MAX_BYTES
        self._waivers = dict(gate.DECLARATION_WAIVERS)

    def tearDown(self) -> None:
        gate.CATALOGUE_MAX_BYTES = self._catalogue
        gate.DECLARATION_MAX_BYTES = self._declaration
        gate.DECLARATION_WAIVERS = self._waivers

    def test_real_catalogue_is_within_its_own_ceiling(self):
        self.assertEqual(run(), 0, "the committed ceilings do not describe the tree")

    def test_the_ceilings_are_pinned_not_slack(self):
        # A ceiling with room to spare is not a ratchet: the surface could grow
        # for free until it hit the slack. One byte below today's size must fail.
        gate.CATALOGUE_MAX_BYTES = self._catalogue - 1
        self.assertEqual(run(), 1, "catalogue ceiling has slack in it")

    def test_one_byte_of_declaration_growth_fails(self):
        gate.DECLARATION_MAX_BYTES = self._declaration - 1
        code, err = run_capturing_stderr()
        self.assertEqual(code, 1)
        self.assertIn("declares", err)

    def test_the_failure_names_the_offending_tool(self):
        # A gate that says "too big" without saying WHERE sends the next person
        # hunting through 84 declarations.
        gate.DECLARATION_MAX_BYTES = 100
        _, err = run_capturing_stderr()
        self.assertIn("disc_append", err)

    def test_a_waiver_suppresses_only_its_own_tool(self):
        gate.DECLARATION_MAX_BYTES = 100
        gate.DECLARATION_WAIVERS = {"disc_append": "under test"}
        _, err = run_capturing_stderr()
        self.assertNotIn("disc_append declares", err)
        self.assertIn("qa_create_draft", err)

    def test_the_catalogue_is_measured_as_the_client_receives_it(self):
        # Summing per-tool bytes undercounts: the wrapper and separators are
        # shipped too, and that is what an agent actually pays for. The ceiling
        # therefore gates on the whole payload, which must exceed the sum of the
        # declarations — by exactly the envelope.
        tools = gate.load_tools(REPO_ROOT)
        declarations = sum(gate.wire_bytes(tool) for tool in tools)
        payload = gate.wire_bytes({"tools": tools})
        self.assertGreater(payload, declarations, "the envelope was not counted")
        self.assertEqual(gate.CATALOGUE_MAX_BYTES, payload,
                         "the ceiling is not pinned to the payload it gates on")

    def test_the_measurement_counts_the_escaping_the_wire_carries(self):
        # The bug this replaced: measuring with `ensure_ascii=False` under-reported
        # the catalogue by 792 B, because every accented character in a French
        # description travels as a 6-byte \uXXXX escape, not as its 2 bytes of
        # UTF-8. Verified against the live server, the corrected measurement
        # matches `tools/list` byte for byte.
        import json
        accented = {"name": "x", "description": "précédé", "inputSchema": {}}
        self.assertGreater(
            gate.wire_bytes(accented),
            len(json.dumps(accented, ensure_ascii=False).encode()),
            "escaping is not being counted",
        )

    def test_a_declaration_is_measured_whole_not_summed_from_parts(self):
        # The parts omit the JSON envelope — the name, the keys, the braces —
        # which is 45 B on the heaviest tool and is really transmitted.
        tools = gate.load_tools(REPO_ROOT)
        heaviest = max(tools, key=lambda t: gate.wire_bytes(t))
        description, schema, total = gate.declaration_bytes(heaviest)
        self.assertGreater(total, description + schema - 2,
                           "the declaration envelope vanished")

    def test_a_destructive_tool_states_what_it_destroys(self):
        # KT-192 DoD 2. The ratchet caps SIZE; this stops the cheapest way to go
        # green, which is gutting a contract. A destructive tool whose description
        # does not say what is lost is the one a caller invokes confidently and
        # regrets — `workflow_cancel_run` gets this right at 281 B ("DESTRUCTIVE —
        # stops the run + its in-flight agents; completed steps/commits are kept"),
        # while five delete/update tools said only "by id (builtins are protected)".
        destructive = [
            tool for tool in gate.load_tools(REPO_ROOT)
            # `update` is deliberately absent: these patch by load-merge-write, so
            # nothing is lost. Widening the list to them made the guard fire on ten
            # tools and would have diluted it into noise.
            if any(verb in tool["name"] for verb in
                   ("delete", "cancel", "unlink", "purge"))
        ]
        self.assertTrue(destructive, "no destructive tool found — check the verb list")
        thin = [
            tool["name"] for tool in destructive
            if not any(marker in tool["description"].lower() for marker in
                       ("irreversible", "cannot be undone", "destructive", "kept",
                        "not deleted", "unaffected", "keeps", "preserved"))
        ]
        self.assertEqual(
            thin, [],
            "destructive tools that do not state what is lost or kept: "
            f"{thin}. Say the blast radius — a caller cannot infer it from `delete`.",
        )

    def test_every_tool_carries_a_description(self):
        # An undocumented tool is not a saving — it is a tool nobody can use
        # correctly, and the cheapest way to game a byte ceiling.
        empty = [t["name"] for t in gate.load_tools(REPO_ROOT)
                 if not t.get("description", "").strip()]
        self.assertEqual(empty, [], f"tools with no description: {empty}")


if __name__ == "__main__":
    unittest.main()

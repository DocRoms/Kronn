#!/usr/bin/env python3
"""Tests for the MCP catalogue census — KT-192 DoD 0.

The census exists to expose a number nobody was watching, so the failure that
matters is the one where it reports a smaller surface than the real one: a server
that could not be probed counted as free, or a server declared twice counted
twice. Both would make the total wrong in a way that looks plausible.
"""

from __future__ import annotations

import io
import json
import pathlib
import sys
import tempfile
import unittest
from contextlib import redirect_stdout

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import mcp_catalogue_census as census  # noqa: E402

# A stdio server in one line: read stdin, answer tools/list, exit.
FAKE_SERVER = (
    "import sys, json\n"
    "tools=[{'name':'alpha','description':'a'*100,'inputSchema':{'type':'object'}},"
    "{'name':'beta','description':'b'*10,'inputSchema':{}}]\n"
    "for line in sys.stdin:\n"
    "    line=line.strip()\n"
    "    if not line: continue\n"
    "    m=json.loads(line)\n"
    "    if m.get('id')==1:\n"
    "        print(json.dumps({'jsonrpc':'2.0','id':1,'result':{}}), flush=True)\n"
    "    elif m.get('id')==2:\n"
    "        print(json.dumps({'jsonrpc':'2.0','id':2,'result':{'tools':tools}}), flush=True)\n"
)

BROKEN_SERVER = "import sys\nsys.stderr.write('Error: No API key\\n')\n"


def config(tmp: pathlib.Path, servers: dict) -> pathlib.Path:
    (tmp / ".mcp.json").write_text(json.dumps({"mcpServers": servers}), encoding="utf-8")
    return tmp


class ProbingAServer(unittest.TestCase):
    def test_a_working_server_reports_its_tools_and_their_bytes(self):
        row = census.probe("fake", {"command": sys.executable, "args": ["-c", FAKE_SERVER]}, 20)
        self.assertIn("tools", row, row)
        names = sorted(t["name"] for t in row["tools"])
        self.assertEqual(names, ["alpha", "beta"])
        alpha = next(t for t in row["tools"] if t["name"] == "alpha")
        self.assertGreater(alpha["bytes"], 100, "the description bytes were not counted")

    def test_a_server_with_no_command_is_unmeasured_not_empty(self):
        # An http/sse server cannot be spawned. Reporting it with zero tools would
        # say its surface is free.
        row = census.probe("remote", {"url": "https://example.com/mcp"}, 5)
        self.assertNotIn("tools", row)
        self.assertIn("transport", row["unmeasured"])

    def test_a_server_that_fails_carries_the_reason(self):
        # "Unmeasured" without a reason is not actionable — a missing API key and a
        # crash need different responses.
        row = census.probe(
            "broken", {"command": sys.executable, "args": ["-c", BROKEN_SERVER]}, 20
        )
        self.assertNotIn("tools", row)
        self.assertIn("No API key", row["unmeasured"])

    def test_a_missing_binary_is_unmeasured_not_a_crash(self):
        row = census.probe("absent", {"command": "definitely-not-a-real-binary-xyz"}, 5)
        self.assertNotIn("tools", row)
        self.assertIn("PATH", row["unmeasured"])


class LoadingTheDeclarations(unittest.TestCase):
    def test_a_server_declared_in_both_scopes_is_counted_once(self):
        # THE inflation case. A name declared twice is ONE server for the client,
        # so double counting would overstate the very total this exposes — as wrong
        # as understating it.
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / ".claude").mkdir()
            (root / ".mcp.json").write_text(
                json.dumps({"mcpServers": {"shared": {"command": "a"}}}), encoding="utf-8"
            )
            (root / ".claude" / "mcp.json").write_text(
                json.dumps({"mcpServers": {"shared": {"command": "b"}}}), encoding="utf-8"
            )
            servers = census.load_servers(root)
        self.assertEqual(list(servers), ["shared"])
        # Project scope wins: the first file read is the authority.
        self.assertEqual(servers["shared"]["command"], "a")

    def test_an_unreadable_config_is_skipped_without_killing_the_census(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / ".mcp.json").write_text("{ not json", encoding="utf-8")
            self.assertEqual(census.load_servers(root), {})

    def test_no_config_at_all_yields_nothing_rather_than_failing(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(census.load_servers(pathlib.Path(tmp)), {})


class Reporting(unittest.TestCase):
    def test_the_total_is_announced_as_a_floor_when_something_is_unmeasured(self):
        # The one sentence that keeps the number honest: with a server unmeasured,
        # the printed total is a lower bound, not the surface.
        rows = [
            {"server": "ok", "tools": [{"name": "a", "bytes": 100}]},
            {"server": "nope", "unmeasured": "no API key"},
        ]
        out = io.StringIO()
        with redirect_stdout(out):
            census.report(rows)
        text = out.getvalue()
        self.assertIn("floor", text)
        self.assertIn("nope", text)
        self.assertIn("no API key", text)

    def test_a_fully_measured_census_claims_no_floor(self):
        # Otherwise the caveat appears always and stops carrying information.
        rows = [{"server": "ok", "tools": [{"name": "a", "bytes": 100}]}]
        out = io.StringIO()
        with redirect_stdout(out):
            census.report(rows)
        self.assertNotIn("floor", out.getvalue())

    def test_unmeasured_servers_are_not_summed_as_zero(self):
        # A zero would be indistinguishable from a server that genuinely declares
        # nothing, and the total would look complete.
        rows = [
            {"server": "ok", "tools": [{"name": "a", "bytes": 500}]},
            {"server": "nope", "unmeasured": "crashed"},
        ]
        out = io.StringIO()
        with redirect_stdout(out):
            census.report(rows)
        text = out.getvalue()
        self.assertIn("500", text)
        self.assertIn("UNMEASURED (1)", text)

    def test_the_heaviest_declarations_are_named_with_their_server(self):
        # "Something is big" is not actionable without knowing which server owns it.
        rows = [
            {"server": "big", "tools": [{"name": "huge", "bytes": 9000}]},
            {"server": "small", "tools": [{"name": "tiny", "bytes": 10}]},
        ]
        out = io.StringIO()
        with redirect_stdout(out):
            census.report(rows)
        text = out.getvalue()
        self.assertIn("big · huge", text)


class EndToEnd(unittest.TestCase):
    def test_a_census_over_a_real_fake_server_measures_it(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = config(
                pathlib.Path(tmp),
                {"fake": {"command": sys.executable, "args": ["-c", FAKE_SERVER]}},
            )
            rows = census.census(root, timeout=20, only=[])
        self.assertEqual(len(rows), 1)
        self.assertIn("tools", rows[0])
        self.assertEqual(len(rows[0]["tools"]), 2)


if __name__ == "__main__":
    unittest.main()

#!/usr/bin/env python3
"""Tests for the anonymised telemetry export — KT-190 DoD 6.

An export leaks by including one field too many, and that failure is silent: the
file is produced, looks right, and carries someone's repository paths. So most of
these tests assert what is NOT in the output, and one of them reads the live
table schema so a column added tomorrow cannot slip out by default.

Run: python3 backend/scripts/test_telemetry_export.py
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import sqlite3
import tempfile
import unittest

_SCRIPT = pathlib.Path(__file__).with_name("telemetry_export.py")


def _load():
    spec = importlib.util.spec_from_file_location("telemetry_export", _SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SECRET_CWD = "/Users/someone/Repositories/acme-private-monorepo"


def build_db(path: pathlib.Path) -> None:
    conn = sqlite3.connect(path)
    conn.executescript(
        """
        CREATE TABLE discussion_sessions (
            id INTEGER PRIMARY KEY, disc_id TEXT, agent_type TEXT,
            session_id TEXT, conversation_id TEXT);
        CREATE TABLE cli_session_telemetry (
            cli_session_pk INTEGER PRIMARY KEY, vendor TEXT, provenance TEXT,
            input_tokens INTEGER, cache_creation_tokens INTEGER,
            cache_read_tokens INTEGER, output_tokens INTEGER,
            measured_responses INTEGER, models_json TEXT,
            window_start TEXT, window_end TEXT, vendor_cost_usd REAL,
            read_offset INTEGER, updated_at TEXT);
        """
    )
    conn.execute(
        "INSERT INTO discussion_sessions VALUES (1, 'disc-secret-name', 'ClaudeCode',"
        " ?, 'conv-abc')",
        (SECRET_CWD,),
    )
    conn.execute(
        "INSERT INTO cli_session_telemetry VALUES "
        "(1, 'claude-code', 'claude-code-transcript', 16826, 61095483,"
        " 4077307836, 5367306, 7640, '{\"claude-opus-5\":5126}',"
        " '2026-07-27T18:06:48Z', '2026-08-05T05:47:43Z', 12.34,"
        " 61869611, '2026-08-05T05:47:44Z')"
    )
    # A Vibe row: no cache split, and a vendor cost that must not leave.
    conn.execute(
        "INSERT INTO discussion_sessions VALUES (2, 'disc-secret-name', 'Vibe',"
        " 'sess-2', 'conv-def')"
    )
    conn.execute(
        "INSERT INTO cli_session_telemetry VALUES "
        "(2, 'vibe', 'vibe-session-meta', 14126817, NULL, NULL, 39907, 152,"
        " '{\"mistral-medium-3.5\":152}', '2026-07-26T15:46:42Z',"
        " '2026-07-26T17:12:12Z', 21.489528, 0, '2026-07-26T17:12:13Z')"
    )
    conn.commit()
    conn.close()


class ExportTests(unittest.TestCase):
    def setUp(self):
        self.mod = _load()
        self.dir = tempfile.TemporaryDirectory()
        self.db = pathlib.Path(self.dir.name) / "k.db"
        build_db(self.db)
        self.payload = self.mod.export(str(self.db), "salt-1")
        self.blob = json.dumps(self.payload, ensure_ascii=False)

    def tearDown(self):
        self.dir.cleanup()

    # ── what must not leave ────────────────────────────────────────

    def test_no_filesystem_path_leaves(self):
        # The session_id column happens to hold a working directory here, which
        # names both the person and their employer's projects.
        self.assertNotIn(SECRET_CWD, self.blob)
        self.assertNotIn("acme-private-monorepo", self.blob)

    def test_no_raw_room_identifier_leaves(self):
        self.assertNotIn("disc-secret-name", self.blob)

    def test_no_vendor_conversation_id_leaves(self):
        # Correlatable with the vendor's own logs, which the reader has no
        # business joining against.
        self.assertNotIn("conv-abc", self.blob)
        self.assertNotIn("conv-def", self.blob)

    def test_no_vendor_cost_leaves(self):
        # Derived from negotiated per-million prices. Checked against the SESSION
        # rows only: the name does appear under `withheld`, on purpose — the
        # export has to say what it removed.
        self.assertNotIn("21.489528", self.blob)
        for record in self.payload["sessions"]:
            self.assertNotIn("vendor_cost_usd", record)

    def test_no_read_offset_or_exact_update_time_leaves(self):
        # A byte position inside a private transcript, and the wall clock of
        # someone's working hours.
        self.assertNotIn("61869611", self.blob)
        self.assertNotIn("05:47:44", self.blob)

    # ── what must survive, or the export is pointless ──────────────

    def test_the_counters_survive_separately(self):
        first = self.payload["sessions"][0]
        self.assertEqual(first["input_tokens"], 16826)
        self.assertEqual(first["cache_read_tokens"], 4077307836)
        self.assertEqual(first["output_tokens"], 5367306)

    def test_provenance_and_vendor_survive(self):
        first = self.payload["sessions"][0]
        self.assertEqual(first["provenance"], "claude-code-transcript")
        self.assertEqual(first["vendor"], "claude-code")

    def test_absence_is_stated_not_left_to_inference(self):
        # A consumer computing a cache ratio must not read null as zero.
        vibe = next(s for s in self.payload["sessions"] if s["vendor"] == "vibe")
        self.assertIsNone(vibe["cache_read_tokens"])
        self.assertEqual(sorted(vibe["unmeasured"]), ["cache_creation", "cache_read"])

    def test_the_note_warns_that_traffic_is_not_cost(self):
        # Without this, a reader would divide by the wrong number by default.
        self.assertIn("62x", self.payload["note"])
        self.assertIn("NOT zero", self.payload["note"])

    # ── pseudonyms: groupable, not reversible, not joinable ────────

    def test_rows_from_one_room_share_a_pseudonym(self):
        rooms = {s["room"] for s in self.payload["sessions"]}
        self.assertEqual(len(rooms), 1, "same room produced different pseudonyms")

    def test_a_different_salt_produces_different_pseudonyms(self):
        other = self.mod.export(str(self.db), "salt-2")
        self.assertNotEqual(
            self.payload["sessions"][0]["room"], other["sessions"][0]["room"],
            "two exports could be joined together",
        )

    def test_the_pseudonym_is_not_the_identifier(self):
        self.assertNotEqual(self.payload["sessions"][0]["room"], "disc-secret-name")

    # ── the allow-list must stay an allow-list ────────────────────

    def test_every_table_column_is_either_allowed_or_explicitly_withheld(self):
        # THE test of this file. A deny-list stays correct until someone adds a
        # column, and then leaks silently. This forces a decision per column.
        conn = sqlite3.connect(self.db)
        columns = {
            row[1] for row in conn.execute("PRAGMA table_info(cli_session_telemetry)")
        }
        conn.close()
        classified = set(self.mod.TELEMETRY_COLUMNS) | set(self.mod.WITHHELD_COLUMNS)
        unclassified = sorted(columns - classified)
        self.assertEqual(
            unclassified, [],
            f"columns neither allowed nor withheld — decide before exporting: "
            f"{unclassified}",
        )

    def test_allowed_and_withheld_do_not_overlap(self):
        overlap = set(self.mod.TELEMETRY_COLUMNS) & set(self.mod.WITHHELD_COLUMNS)
        self.assertEqual(overlap, set(), f"contradictory classification: {overlap}")

    def test_a_database_without_the_table_reports_unavailable(self):
        # Hit for real against a pre-migration database. An empty export would
        # read as "nothing was spent", which is the opposite of the truth.
        empty = pathlib.Path(self.dir.name) / "old.db"
        sqlite3.connect(empty).close()
        payload = self.mod.export(str(empty), "salt-1")
        self.assertEqual(payload["status"], "unavailable")
        self.assertEqual(payload["sessions"], [])
        self.assertIn("NOT the same as nothing being spent", payload["reason"])

    def test_a_real_sql_error_still_raises(self):
        # Only a missing table degrades. Swallowing every OperationalError would
        # turn a corrupt database into a clean empty export.
        missing = pathlib.Path(self.dir.name) / "nope.db"
        with self.assertRaises(sqlite3.OperationalError):
            self.mod.export(str(missing), "salt-1")

    def test_the_export_names_what_it_withheld(self):
        # A reader must know something was removed, or they will treat the file
        # as the whole truth.
        self.assertIn("vendor_cost_usd", self.payload["withheld"])
        self.assertIn("read_offset", self.payload["withheld"])


if __name__ == "__main__":
    unittest.main()

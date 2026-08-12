#!/usr/bin/env python3
"""Tests for the telemetry backfill — KT-190.

The defect this file exists for was found by running the tool, not by reading it:
one CLI conversation had joined SEVEN rooms, so a naive backfill wrote its
4 308 007 075 tokens against seven session rows — over 30 billion, presented as
a measurement. A backfill that inflates is worse than no backfill, because the
number looks authoritative.

Run: python3 backend/scripts/test_telemetry_backfill.py
"""

from __future__ import annotations

import importlib.util
import json
import pathlib
import sqlite3
import tempfile
import unittest
from unittest import mock

_SCRIPT = pathlib.Path(__file__).with_name("telemetry_backfill.py")


def _load():
    spec = importlib.util.spec_from_file_location("telemetry_backfill", _SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def fake_collector(counters=None, status="measured"):
    """A collector stub: the transcript reading itself is tested elsewhere."""
    return type("C", (), {
        "collect_for_session": staticmethod(lambda vendor, key, since_offset=0: (
            {
                "status": status,
                "vendor": "claude-code",
                "provenance": "claude-code-transcript",
                "counters": counters if counters is not None
                else {"input": 1, "cache_creation": 2, "cache_read": 3, "output": 4},
                "measured_responses": 10,
                "models": {"claude-opus-5": 10},
                "window_start": "2026-07-27T18:06:48Z",
                "window_end": "2026-08-05T11:41:22Z",
                "next_offset": 999,
            } if status == "measured" else
            {"status": "not_measured", "reason": "no transcript found"}
        )),
    })


class BackfillTests(unittest.TestCase):
    def setUp(self):
        self.mod = _load()
        self.dir = tempfile.TemporaryDirectory()
        self.db = pathlib.Path(self.dir.name) / "k.db"
        conn = sqlite3.connect(self.db)
        conn.executescript(
            """
            CREATE TABLE discussion_sessions (
                id INTEGER PRIMARY KEY, disc_id TEXT, agent_type TEXT,
                conversation_id TEXT, status TEXT);
            CREATE TABLE cli_session_telemetry (
                cli_session_pk INTEGER PRIMARY KEY, vendor TEXT, provenance TEXT,
                input_tokens INTEGER, cache_creation_tokens INTEGER,
                cache_read_tokens INTEGER, output_tokens INTEGER,
                measured_responses INTEGER, models_json TEXT,
                window_start TEXT, window_end TEXT, vendor_cost_usd REAL,
                read_offset INTEGER NOT NULL DEFAULT 0, updated_at TEXT);
            """
        )
        conn.commit()
        conn.close()

    def tearDown(self):
        self.dir.cleanup()

    def _sessions(self, rows):
        conn = sqlite3.connect(self.db)
        conn.executemany(
            "INSERT INTO discussion_sessions (id, disc_id, agent_type,"
            " conversation_id, status) VALUES (?, ?, ?, ?, 'active')", rows
        )
        conn.commit()
        conn.close()

    def _plan(self, counters=None, status="measured"):
        with mock.patch.object(self.mod, "_load_collector",
                               return_value=fake_collector(counters, status)):
            return self.mod.plan(str(self.db))

    # ── the defect that made this file necessary ───────────────────

    def test_one_conversation_across_many_rooms_is_counted_once(self):
        # THE test. Seven joins of one CLI conversation must not become seven
        # copies of its cost.
        self._sessions([(i, f"d-{i}", "ClaudeCode", "conv-shared") for i in range(1, 8)])
        result = self._plan()
        self.assertEqual(len(result["measured"]), 1, "cost was multiplied")
        self.assertEqual(result["measured"][0]["session"], 1, "owner must be deterministic")
        self.assertEqual(len(result["skipped"]), 6)

    def test_the_duplicates_name_the_session_that_holds_their_cost(self):
        # Silently dropping them would read as "these were free".
        self._sessions([(1, "d-1", "ClaudeCode", "conv-shared"),
                        (2, "d-2", "ClaudeCode", "conv-shared")])
        result = self._plan()
        why = result["skipped"][0]["why"]
        self.assertIn("session 1", why)
        self.assertIn("cannot be split between rooms", why)

    def test_distinct_conversations_are_each_counted(self):
        self._sessions([(1, "d-1", "ClaudeCode", "conv-a"),
                        (2, "d-2", "ClaudeCode", "conv-b")])
        self.assertEqual(len(self._plan()["measured"]), 2)

    # ── absence stays absence ──────────────────────────────────────

    def test_a_session_without_a_conversation_id_is_skipped_not_zeroed(self):
        self._sessions([(1, "d-1", "ClaudeCode", None)])
        result = self._plan()
        self.assertEqual(result["measured"], [])
        self.assertIn("no conversation id", result["skipped"][0]["why"])

    def test_a_vendor_with_no_retroactive_collector_is_skipped(self):
        # Vibe is excluded on purpose: its snapshot is keyed to a working
        # directory, and an old cwd may since have been reused by another
        # session — attributing that would be someone else's tokens.
        self._sessions([(1, "d-1", "Codex", "conv-a"), (2, "d-2", "Vibe", "conv-b")])
        result = self._plan()
        self.assertEqual(result["measured"], [])
        self.assertEqual(len(result["skipped"]), 2)

    def test_a_missing_transcript_is_skipped_not_written_as_zero(self):
        self._sessions([(1, "d-1", "ClaudeCode", "conv-gone")])
        result = self._plan(status="not_measured")
        self.assertEqual(result["measured"], [])
        self.assertIn("no transcript", result["skipped"][0]["why"])

    def test_an_absent_counter_is_written_as_null(self):
        self._sessions([(1, "d-1", "ClaudeCode", "conv-a")])
        result = self._plan(counters={"input": 5, "output": 2})
        self.mod.apply(str(self.db), result["measured"])
        conn = sqlite3.connect(self.db)
        row = conn.execute(
            "SELECT input_tokens, cache_read_tokens FROM cli_session_telemetry"
        ).fetchone()
        conn.close()
        self.assertEqual(row[0], 5)
        self.assertIsNone(row[1], "an unpublished counter became zero")

    # ── writing must be safe to repeat ─────────────────────────────

    def test_nothing_is_written_without_apply(self):
        self._sessions([(1, "d-1", "ClaudeCode", "conv-a")])
        self._plan()
        conn = sqlite3.connect(self.db)
        count = conn.execute("SELECT COUNT(*) FROM cli_session_telemetry").fetchone()[0]
        conn.close()
        self.assertEqual(count, 0)

    def test_running_twice_does_not_duplicate_rows(self):
        self._sessions([(1, "d-1", "ClaudeCode", "conv-a")])
        measured = self._plan()["measured"]
        self.mod.apply(str(self.db), measured)
        self.mod.apply(str(self.db), measured)
        conn = sqlite3.connect(self.db)
        count = conn.execute("SELECT COUNT(*) FROM cli_session_telemetry").fetchone()[0]
        conn.close()
        self.assertEqual(count, 1)

    def test_it_never_rewinds_a_cursor_the_live_collector_advanced(self):
        # The live collector may be far ahead. Lowering its offset would make it
        # re-read, and re-count, a span already measured.
        self._sessions([(1, "d-1", "ClaudeCode", "conv-a")])
        conn = sqlite3.connect(self.db)
        conn.execute(
            "INSERT INTO cli_session_telemetry (cli_session_pk, vendor, provenance,"
            " read_offset, updated_at) VALUES (1, 'claude-code', 'live', 50000, 'now')"
        )
        conn.commit()
        conn.close()

        self.mod.apply(str(self.db), self._plan()["measured"])
        conn = sqlite3.connect(self.db)
        offset = conn.execute(
            "SELECT read_offset FROM cli_session_telemetry WHERE cli_session_pk = 1"
        ).fetchone()[0]
        conn.close()
        self.assertEqual(offset, 50000, "cursor was rewound")

    def test_backfilled_provenance_is_distinguishable_from_live(self):
        # A number recovered after the fact and one measured as it happened are
        # not the same evidence.
        self._sessions([(1, "d-1", "ClaudeCode", "conv-a")])
        result = self._plan()
        self.assertTrue(result["measured"][0]["provenance"].endswith("-backfill"))

    def test_the_first_window_start_is_preserved_on_refresh(self):
        self._sessions([(1, "d-1", "ClaudeCode", "conv-a")])
        measured = self._plan()["measured"]
        self.mod.apply(str(self.db), measured)
        shifted = [dict(measured[0], window_start="2099-01-01T00:00:00Z")]
        self.mod.apply(str(self.db), shifted)
        conn = sqlite3.connect(self.db)
        start = conn.execute(
            "SELECT window_start FROM cli_session_telemetry WHERE cli_session_pk = 1"
        ).fetchone()[0]
        conn.close()
        self.assertEqual(start, "2026-07-27T18:06:48Z")


if __name__ == "__main__":
    unittest.main()
